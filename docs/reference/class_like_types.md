# クラスライク型リファレンス (操作モールド中心)

## 概要

Taida のユーザー定義型 (旧 TypeDef / Mold 継承 / Error 継承) は、E30 (gen-E 破壊的変更) で **クラスライク型** の単一構文に統合されました。本リファレンスは class-like 型の中でも特に **操作モールド** (Mold[T] を継承して定義される操作型) の構文・標準モールド一覧・型シグネチャを扱います。

- class-like 単一構文 (`Name[?type-args] [=> Parent] = @(...)`) の概念ガイドは [`../guide/04_class_like.md`](../guide/04_class_like.md) を参照してください
- 操作モールド (Mold) の概念ガイドは [`../guide/05_molding.md`](../guide/05_molding.md) を参照してください
- 旧来の 3 系統 (TypeDef / Mold 継承 / Error 継承) は、本リファレンスのクラスライク統一構文に統合されています

> **PHILOSOPHY.md — III.** カタめたいなら、鋳型を作りましょう

> **位置付け** — このファイルは旧 `mold_types.md` の後継です。class-like 統一概念のもとで「操作モールド」を扱うリファレンスとして配置しています。Mold 基底クラスのヘッダ規則 / `solidify` / `unmold` / 標準モールド全種 / 型シグネチャ一覧を扱います。`[E1407]` / `[E1410]` などの診断コードの意味は `docs/reference/diagnostic_codes.md` と同期して定義されます。

---

## 設計原則: `[]` と `()` の役割分担

```rust
// AST 定義
MoldInst(String, Vec<Expr>, Vec<BuchiField>, Span)
//       名前    [位置引数]   (名前付き設定)
```

| | `[]` 位置引数 | `()` 名前付き設定 |
|---|---|---|
| **役割** | 必須引数（何を / 何で） | オプション設定（どうやって） |
| **名前** | なし（位置で区別） | あり（名前で区別） |
| **省略** | 不可 | 可（デフォルト値あり） |
| **順序** | 固定（`[何を, 何で]`） | 任意 |

---

## class-like 統一構文の構築検査ポリシー

`Name(field <= value, ...)` のクラスライク型コンストラクタ呼び出しは、`@e.32` 以降 **closed-constructor** として検査されます。匿名 `@(...)` リテラルは引き続き open / 構造的な値として扱い、本ポリシーの対象外です。

| ケース | 受理 / 拒否 | 診断 |
|---|---|---|
| 宣言済みフィールドのみで呼び出し | 受理 | — |
| 宣言済みフィールドの省略 | 受理 (型のデフォルト値で充足) | — |
| declare-only 関数フィールドの省略 | 受理 (型の defaultFn が充足) | — |
| 宣言外フィールドを渡す | 拒否 | `[E1406]` |
| 同一フィールドを複数回渡す | 拒否 | `[E1404]` |
| 宣言済みフィールドに型不一致な値を渡す | 拒否 | `[E1506]` |
| メソッドフィールドに値を渡す | 拒否 | `[E1407]` |
| Error 派生で `type` に同名 literal を渡す | 受理 (idempotent legacy aid) | — |
| Error 派生で `type` に別 literal / 非 literal を渡す | 拒否 | `[E1408]` |
| 匿名 `@(...)` への余分フィールド | 受理 (open shape を維持) | — |

```taida fragment
Pilot = @(name: Str, age: Int)

Pilot(name <= "Rei", age <= 14)            // OK
Pilot(name <= "Asuka")                     // OK (age はデフォルト 0)
Pilot(name <= "Rei", typo_age <= 14)       // [E1406]
Pilot(name <= "Rei", name <= "Asuka")      // [E1404]
Pilot(name <= "Rei", age <= "fourteen")    // [E1506]

@(x <= 1, y <= "extra", z <= true)         // OK (匿名 pack は open)

Error => MyError = @(field: Str, message: Str)
MyError(type <= "MyError", field <= "x", message <= "y")  // OK
MyError(type <= "AppError", message <= "y")               // [E1408]
MyError(feild <= "typo", message <= "y")                  // [E1406]
```

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ
>
> 構造的サブタイピングは代入 / 引数渡し / 戻り値の互換規則として維持されます。コンストラクタの closed-form 検査は型の identity を立てる際の AI typo 防止と silent breakage 抑止に集中し、structural compat 規則とは独立に動作します。

---

## Mold基底クラス

操作モールドは class-like 単一構文 (`Name[?type-args] [=> Parent] = @(...)`) を `Mold[...]` を親型として用いる特殊化です。すべてのモールディング型は `Mold[...]` を継承して定義します (E30 統一構文では `=> Mold[T]` の形になり、ぶちパック型定義 / Error 型と同じ surface 構文を共有します)。

```taida
// 基本形式
Mold[T] => MyMold[T] = @(
  filling: T
  solidify _ => :V
  unmold _ => :U
  // 追加フィールド
)

// 例
Mold[T] => Result[T, P <= :T => :Bool] = @(throw: Error)  // 述語付き操作モールド
```

header 記法:

- `T` = 型変数
- `:Int` = concrete type
- `T <= :Int` = concrete type 制約付き型変数
- `Mold[...]` は親ヘッダー、`Name[...]` は子ヘッダー
- `Mold[...]` の親側は常に 1 slot のまま保つ
- 追加 slot は子ヘッダー側にだけ書く
- 子ヘッダーは親ヘッダーを exact prefix として保持し、末尾にだけ slot を追加できる

ヘッダースロット束縛規則:

1. `Mold[...]` の1つ目は常に `filling`
2. 2つ目以降は、`@(...)` の「デフォルト値なしフィールド」に宣言順で対応
3. `T` だけでなく `:Int` のような具象型スロットも1つのフィールドスロットを消費する
4. どれか1つでも束縛先が足りないと `E1401`
5. `Name[...]` を明示する場合は、`Mold[...]` を exact prefix に保ったまま、同数またはそれ以上の slot 数でなければ `E1407`

例:

```taida fragment
Mold[:Int] => IntBox = @()
IntBox[1]()       // OK
IntBox["x"]()     // E1408: 具象型ヘッダー型不一致

Mold[:Int] => IntPair[:Int, T] = @(
  second: T
)
IntPair[1, "x"]() // OK

Mold[:Int] => Broken[:Int, T] = @()
// E1401: 2つ目のヘッダースロットに対応するフィールドが無い
```

### solidify / unmold（正式仕様）

| フック | デフォルト | 役割 |
|---|---|---|
| `solidify` | `self` を返す | `Name[args]()` が何の型として固まるか（Is-A）を決定 |
| `unmold` | `filling` を返す | `]=>` / `<=[` / `.unmold()` で取り出す値を決定 |

`[]` / `()` 束縛規則:

1. 1つ目の `[]` は常に `filling`
2. 2つ目以降の `[]` はデフォルト値なしフィールドへ宣言順に束縛
3. `()` はデフォルト値ありフィールドのみ指定可能
4. 規則違反はコンパイルエラー（不足/過多/取り違え/未定義オプション）
5. カスタムモールド定義時、追加型引数に対応する束縛先（デフォルト値なしフィールド）が無ければコンパイルエラー
6. 通常フィールドは `field: Type` または `field <= value` のどちらかが必須（`field` 単独はコンパイルエラー）

`Name[args]()` の評価順序:

1. `[]` を `filling` とデフォルト値なしフィールドへ順に束縛
2. `()` をデフォルト値ありフィールドへ束縛
3. `solidify` を評価
4. 式 `Name[args]()` の型は `solidify` の戻り値型

演算子の意味:

- `Name[args]() => x`: `solidify` の結果を代入
- `Name[args]() ]=> x`: `solidify` 結果に `unmold` を適用して代入

例:

```taida
Lax[42]() => boxed      // boxed: Lax[Int]（default solidify）
Lax[42]() ]=> value     // value: Int

Int["123"]() => parsed  // parsed: Lax[Int]（solidify override）
Int["123"]() ]=> num    // num: Int
```

---

## 文字列モールド

### Upper[str]

すべての文字を大文字に変換します。

```taida fragment
Upper["hello"]()              // "HELLO"
str => Upper[_]() => result
```

### Lower[str]

すべての文字を小文字に変換します。

```taida fragment
Lower["HELLO"]()              // "hello"
str => Lower[_]() => result
```

### Trim[str]

空白を除去します。オプションで除去方向を制御できます。

```taida
Trim["  hello  "]()                    // "hello"（両端）
Trim["  hello  "](end <= false)        // "hello  "（先頭のみ）
Trim["  hello  "](start <= false)      // "  hello"（末尾のみ）
```

| オプション | デフォルト | 説明 |
|-----------|----------|------|
| `start` | `true` | 先頭の空白を除去 |
| `end` | `true` | 末尾の空白を除去 |

### Split[str, delim]

区切り文字で分割してリストを返します。

```taida fragment
Split["a,b,c", ","]()         // @["a", "b", "c"]
str => Split[_, ","]() => parts
```

### Replace[str, old, new]

部分文字列を置換します。オプションで全置換を制御できます。

```taida
Replace["hello world", "o", "0"]()              // "hell0 world"（最初の1つ）
Replace["hello world", "o", "0"](all <= true)   // "hell0 w0rld"（全部）
```

| オプション | デフォルト | 説明 |
|-----------|----------|------|
| `all` | `false` | `true` で全一致を置換 |

### Slice[str]

指定範囲の部分文字列を返します。

```taida
Slice["hello"](start <= 1, end <= 3)   // "el"
Slice["hello"](start <= 2)             // "llo"
Slice["hello"](end <= 3)               // "hel"
```

| オプション | デフォルト | 説明 |
|-----------|----------|------|
| `start` | `0` | 開始位置 |
| `end` | 文字列長 | 終了位置 |

### CharAt[str, idx]

指定位置の文字を返します。Lax[Str] を返し、範囲外の場合は has_value=false（デフォルト値 ""）。

```taida fragment
CharAt["hello", 0]() ]=> ch   // "h"
CharAt["hello", 4]() ]=> ch   // "o"
```

### Repeat[str, n]

文字列を指定回数繰り返します。

```taida
Repeat["ha", 3]()             // "hahaha"
Repeat["x", 0]()              // ""
```

### Reverse[str]

文字列を逆順にします（リストにも使用可能）。

```taida
Reverse["hello"]()            // "olleh"
```

### Pad[str, len]

指定長になるようパディングします。

```taida
Pad["42", 5](side <= "start")                 // "   42"
Pad["42", 5](side <= "end")                   // "42   "
Pad["42", 5](side <= "start", char <= "0")    // "00042"
```

| オプション | デフォルト | 説明 |
|-----------|----------|------|
| `side` | `"start"` | `"start"` または `"end"` |
| `char` | `" "` | パディング文字 |

---

## 数値モールド

### ToFixed[num, digits]

指定した小数点以下の桁数で文字列に変換します。

```taida
ToFixed[3.14159, 2]()         // "3.14"
ToFixed[42, 2]()              // "42.00"
```

### Abs[num]

絶対値を返します。

```taida
Abs[-5]()                     // 5
Abs[3.14]()                   // 3.14
```

### Floor[num]

小数点以下を切り捨てた整数を返します。

```taida
Floor[3.7]()                  // 3
Floor[-3.7]()                 // -4
```

### Ceil[num]

小数点以下を切り上げた整数を返します。

```taida
Ceil[3.2]()                   // 4
Ceil[-3.2]()                  // -3
```

### Round[num]

四捨五入した整数を返します。

```taida
Round[3.4]()                  // 3
Round[3.5]()                  // 4
```

### Truncate[num]

0方向に切り捨てた整数を返します。

```taida
Truncate[3.7]()               // 3
Truncate[-3.7]()              // -3
```

### Clamp[num, min, max]

指定範囲に収めた値を返します。

```taida
Clamp[5, 0, 10]()             // 5
Clamp[-5, 0, 10]()            // 0
Clamp[15, 0, 10]()            // 10
```

### BitAnd / BitOr / BitXor / BitNot

ビット演算をモールドとして提供します（演算子は追加しません）。

```taida
BitAnd[6, 3]()                 // 2
BitOr[6, 3]()                  // 7
BitXor[6, 3]()                 // 5
BitNot[0]()                    // -1
```

### ShiftL / ShiftR / ShiftRU

`n`（シフト量）が `0..63` のとき成功し、`Lax[Int]` を返します。範囲外は `has_value = false` です。

```taida
ShiftL[1, 40]() ]=> x          // 1099511627776
ShiftRU[-1, 1]() ]=> y         // 9223372036854775807
ShiftL[1, 64]().has_value       // false
```

### ToRadix[int, base]

整数を指定基数（`2..36`）の文字列へ変換します。

```taida
ToRadix[255, 16]() ]=> s       // "ff"
ToRadix[10, 1]().has_value      // false
```

### Int[str, base]

指定基数（`2..36`）で文字列を整数に変換します。基数が範囲外の場合は変換失敗になります。符号は先頭 `+` または `-` で表現します。

```taida fragment
Int["ff", 16]() ]=> n          // 255
Int["FF", 16]() ]=> n          // 255 (大文字も受理)
Int["+ff", 16]() ]=> n         // 255 (+ prefix も受理)
Int["1010", 2]() ]=> n         // 10
Int["77", 8]() ]=> n           // 63
Int["-ff", 16]() ]=> n         // -255
Int["2", 2]().has_value         // false (基数2で "2" は無効)
Int["5", 1]().has_value         // false (基数1は範囲外)
```

### Ordinal[enum] (C18-3)

Enum 値を宣言順の ordinal Int に変換します。Enum 表現力強化 (C18) で追加。

```taida
Enum => HiveState = :Creating :Running :Stopped

Ordinal[HiveState:Running()]()  // 1
Ordinal[HiveState:Creating()]() // 0
Ordinal[HiveState:Stopped()]()  // 2
```

- **引数**: Enum 値ちょうど 1 つ。非 Enum を渡すと runtime error。
- **戻り値**: `Int`（`Lax[Int]` ではない — 変換は常に成功する）。
- **逆方向** (`Int → Enum`): C18 では未実装。別 track の `FromOrdinal[]` で検討中。
- **用途**: 既存 Int カラム / binary wire との互換、`Ordinal[] > 0` のような Int 空間でのしきい値比較、将来の variant 並び替えに備えた explicit 固定。

`.toString()` で得た Str を `Int[]` で parse する workaround は fragile なので使わないでください。

### UInt8 / Bytes / ByteSet / BytesToList

バイト列境界のモールド群です。`Bytes` は不変で、更新は新しい値を返します。

```taida
Bytes[4](fill <= 65) ]=> b      // Bytes[@[65, 65, 65, 65]]
ByteSet[b, 1, 66]() ]=> b2      // Bytes[@[65, 66, 65, 65]]
BytesToList[b2]()                // @[65, 66, 65, 65]
UInt8[255]() ]=> v               // 255
```

### Char / CodePoint / Utf8Encode / Utf8Decode

Unicode scalar value と UTF-8 の相互変換です。`CodePoint` は「1文字の Str」のみ成功します。

```taida
Char[65]() ]=> c                 // "A"
CodePoint["A"]() ]=> cp          // 65
Utf8Encode["pong"]() ]=> raw     // Bytes[@[112, 111, 110, 103]]
Utf8Decode[raw]() ]=> text       // "pong"
bad <= Bytes[@[255]]()
bad ]=> badBytes
Utf8Decode[badBytes]().has_value   // false
```

---

## リストモールド

### Reverse[list]

リストを逆順にします（文字列にも使用可能）。

```taida
Reverse[@[1, 2, 3]]()        // @[3, 2, 1]
```

### Concat[list, other]

2つのリストを結合します。

```taida
Concat[@[1, 2], @[3, 4]]()   // @[1, 2, 3, 4]
```

### Append[list, val]

末尾に要素を追加した新しいリストを返します。

```taida
Append[@[1, 2], 3]()         // @[1, 2, 3]
```

### Prepend[list, val]

先頭に要素を追加した新しいリストを返します。

```taida
Prepend[@[2, 3], 1]()        // @[1, 2, 3]
```

### Join[list, sep]

要素を区切り文字で結合して文字列にします。

```taida
Join[@["a", "b", "c"], ","]()  // "a,b,c"
Join[@[1, 2, 3], "-"]()        // "1-2-3"
```

### Sum[list]

数値リストの合計を返します。

```taida
Sum[@[1, 2, 3]]()            // 6
Sum[@[]]()                    // 0
```

### Sort[list]（統合モールド）

要素をソートします。オプションで降順・キー関数を制御できます。

```taida fragment
Sort[@[3, 1, 2]]()                              // @[1, 2, 3]
Sort[@[3, 1, 2]](reverse <= true)               // @[3, 2, 1]
Sort[pilots](by <= _ p = p.sync_rate)             // キー関数ソート
Sort[pilots](by <= _ p = p.name, reverse <= true)  // キー関数降順
```

| オプション | デフォルト | 説明 |
|-----------|----------|------|
| `reverse` | `false` | `true` で降順 |
| `by` | なし（自然順） | キー抽出関数 |

### Unique[list]（統合モールド）

重複を除去したリストを返します。

```taida fragment
Unique[@[1, 2, 2, 3, 3]]()                      // @[1, 2, 3]
Unique[items](by <= _ x = x.id)                  // キーで重複判定
```

| オプション | デフォルト | 説明 |
|-----------|----------|------|
| `by` | なし（値の等価比較） | キー抽出関数 |

### Flatten[list]

ネストしたリストを1段階フラット化します。

```taida
Flatten[@[@[1, 2], @[3, 4]]]()  // @[1, 2, 3, 4]
```

### Find[list, fn]

条件を満たす最初の要素を Lax で返します。

```taida
Find[@[1, 2, 3, 4], _ x = x > 2]() ]=> val  // 3
Find[@[1, 2], _ x = x > 10]().has_value       // false
```

### FindIndex[list, fn]

条件を満たす最初の要素の位置を返します。見つからない場合は -1。

```taida
FindIndex[@[1, 2, 3, 4], _ x = x > 2]()  // 2
```

### Count[list, fn]

条件を満たす要素数を返します。

```taida
Count[@[1, 2, 3, 4, 5], _ x = x > 2]()  // 3
```

### Take[list, n] / Drop[list, n]

先頭からn個取得/スキップします。

```taida
Take[@[1, 2, 3, 4, 5], 3]()  // @[1, 2, 3]
Drop[@[1, 2, 3, 4, 5], 2]()  // @[3, 4, 5]
```

### TakeWhile[list, fn] / DropWhile[list, fn]

条件を満たす間取得/スキップします。

```taida
TakeWhile[@[1, 2, 3, 4, 5], _ x = x < 4]()  // @[1, 2, 3]
DropWhile[@[1, 2, 3, 4, 5], _ x = x < 3]()  // @[3, 4, 5]
```

### Zip[list, other]

2つのリストをペアにします。短い方に合わせます。

```taida
Zip[@[1, 2, 3], @["a", "b", "c"]]() ]=> pairs
// pairs: @[@(first <= 1, second <= "a"), @(first <= 2, second <= "b"), ...]
```

### Enumerate[list]

インデックスを付与したリストを返します。

```taida
Enumerate[@["a", "b", "c"]]() ]=> indexed
// indexed: @[@(index <= 0, value <= "a"), @(index <= 1, value <= "b"), ...]
```

### Filter[list, fn]

条件を満たす要素を抽出します。

```taida
isEven x =
  Mod[x, 2]() ]=> r
  r == 0
=> :Bool
Filter[@[1, 2, 3, 4], isEven]() ]=> evens  // @[2, 4]
```

### Map[list, fn]

各要素に関数を適用します。

```taida
Map[@[1, 2, 3], _ x = x * 2]() ]=> doubled  // @[2, 4, 6]
```

### Fold[list, init, fn]

左から畳み込みます。

```taida
Fold[@[1, 2, 3], 0, _ acc x = acc + x]() ]=> sum  // 6
```

### Foldr[list, init, fn]

右から畳み込みます。

```taida
Foldr[@["a", "b", "c"], "", _ acc x = x + acc]() ]=> concat  // "abc"
```

---

## 演算モールディング型

### Div[x, y]

除算を行い、Lax を返します。ゼロ除算の場合は `has_value = false`。

```taida fragment
Div[10, 3]() ]=> result   // 3
Div[10, 0]() ]=> result   // 0 (ゼロ除算: デフォルト値)
Div[10, 0]().has_value      // false
```

### Mod[x, y]

剰余を計算し、Lax を返します。ゼロ除算の場合は `has_value = false`。

```taida
Mod[10, 3]() ]=> result   // 1
Mod[10, 0]().has_value      // false
```

---

## 条件モールディング型

### If[cond, then, else]

```taida fragment
If[condition, then_value, else_value]() => T
```

2 分岐の条件式。`condition` を評価し、truthy なら `then_value`、falsy なら `else_value` を返します。

- 選択されなかった枝は評価しません（短絡評価）
- パイプラインで `_` を使って前段の値を参照できます
- ネスト可能: `If[cond, If[cond2, a, b](), c]()`

```taida fragment
If[x > 0, "positive", "negative"]()
150 => If[_ > 100, 100, _]()   // clamp: 100
If[true, If[false, 1, 2](), 3]()   // 2
```

---

## 型比較モールディング型

### TypeIs[value, :TypeName]

```taida
TypeIs[value, :TypeName]() => Bool
```

値の実行時の型が指定した型名と一致するかを判定します。

対応する型リテラル: `:Int`, `:Float`, `:Num`, `:Bool`, `:Str`, `:Bytes`, `:Error`, `:NamedType`

Enum variant の判定: `TypeIs[value, EnumName:Variant]()` で特定の variant かを判定します。

```taida
TypeIs[42, :Int]()                // true
TypeIs["hello", :Str]()          // true
TypeIs[42, :Num]()               // true
TypeIs[Status:Ok(), Status:Ok]() // true
```

### TypeExtends[:TypeA, :TypeB]

```taida
TypeExtends[:TypeA, :TypeB]() => Bool
```

TypeA が TypeB と同じ型か、TypeB のサブタイプかを判定します。コンパイル時に解決可能です。

```taida
TypeExtends[:Int, :Num]()        // true
TypeExtends[:Dog, :Animal]()     // true（Dog が Animal を継承している場合）
TypeExtends[:Str, :Int]()        // false
```

### TypeName[value]

```taida
TypeName[value]() => Str
```

値の type identity を文字列で返します。class-like 系統 (Mold / Error / TypeDef / Inheritance) のインスタンスでは継承位置の名前 (`__type` 相当) を、enum variant では variant 名を、プリミティブ (`Int` / `Float` / `Bool` / `Str` / `Bytes`) では型名を返します。失敗ケースが無いため戻り値は `Str` 直接 (`Lax` ではありません)。

```taida fragment
TypeName[42]()                                                       // "Int"
TypeName["hello"]()                                                  // "Str"
TypeName[3.14]()                                                     // "Float"
TypeName[AppError(type <= "AppError", message <= "")]()              // "AppError"
TypeName[DbError(type <= "DbError", message <= "", ...)]()           // "DbError"  (継承位置名)
TypeName[Status:Ok()]()                                              // "Ok"        (variant 名)
TypeName[@(a <= 1)]()                                                // ""          (plain buchi-pack)

// パイプライン互換
err => TypeName[_]() => name
```

`typeof(x)` 関数 (declared static type を返す compile-time helper、`docs/reference/standard_library.md` の「ユーティリティ」を参照) との違い:

- `typeof(x)` は **compile-time** に解決される declared static type 名
- `TypeName[x]()` は **value-time** の class-like 継承位置 (`__type` 相当) や enum variant 名

`__type` フィールドへの直接アクセス (`err.__type` 等) は `[E1960]` で reject されます。継承位置や variant 名を読みたい場合は `TypeName[x]()` を使ってください。

---

## 型変換モールディング型

型変換モールドが `Lax[...]` を返す挙動は、`solidify` オーバーライドで定義される言語仕様です（専用のコンパイラ特別扱いではない）。

### Int[x]

値を整数に変換し、Lax を返します。文字列から整数への変換（Str → Int）の正規経路です。

```taida fragment
Int["123"]() ]=> num   // 123
Int["abc"]() ]=> num   // 0 (変換失敗: デフォルト値)
Int[3.14]() ]=> num    // 3
Int["+5"]() ]=> num    // 5 (符号付き文字列も受理)
```

受理される文字列: 先頭に `+` または `-` を含むオプションの符号、続いて1桁以上の数字（`0-9`）。空文字列、小数点を含む文字列、先頭/末尾の空白、数字以外の文字を含む文字列は変換失敗になります。

`Int[str, base]()` で基数を指定した変換も可能です（詳細は [数値モールド](#intstr-base) を参照）。

### Float[x]

値を浮動小数点数に変換し、Lax を返します。

```taida fragment
Float["3.14"]() ]=> val  // 3.14
Float[42]() ]=> val      // 42.0
```

### Str[x]

値を文字列に変換し、Lax を返します。`x` はプリミティブ（Int / Float / Bool / Str）だけでなく、List / ぶちパック / 他の Lax など任意の Taida 値を受け取れます。戻り値は常に `Lax[Str]`（失敗条件が無いため `has_value` は常に `true`）。

```taida fragment
Str[42]() ]=> text       // "42"
Str[3.14]() ]=> text     // "3.14"
Str[3.0]() ]=> text      // "3"       — 整数値のFloatは小数点以下を落とす
Str[-5.0]() ]=> text     // "-5"
Str[true]() ]=> text     // "true"
Str[@[1, 2, 3]]() ]=> t  // "@[1, 2, 3]"
Str[@(a <= 1)]() ]=> t   // "@(a <= 1)"
Str[Int[3.0]()]() ]=> t  // 内側 Lax の full-form 表示:
                          // `@(has_value <= true, __value <= 3, __default <= 0, __type <= "Lax")`
```

**パリティ保証**: Interpreter / JS / Native / WASM-wasi の 4 バックエンドで同一出力を返します。Interpreter が参照実装であり、その `Str` の変換規則は以下のとおりです:

- `Int` → 10進表記 (`42` → `"42"`)
- `Float` → Rust `f64::to_string` 相当の最短往復表記（整数値は `.0` を落とす: `3.0` → `"3"`、`3.14` → `"3.14"`）
- `Bool` → `"true"` / `"false"`
- `Str` → そのまま（クォート無し）
- それ以外 (List / ぶちパック / Lax / Result / Async / HashMap / Set / Gorillax / TODO / Molten …) → 各値の display 文字列（ぶちパックは `__` で始まる内部フィールドも含む full-form、nested な HashMap / Set / ぶちパックも再帰的に full-form に展開）

`Str[Gorillax[v]()]()` は C24-A 以降、WASM-wasi でも第 1 フィールド名を `has_value` に統一済みです。`tests/c23_str_parity.rs` の skip list は空で、Gorillax / Stream を含む `Str[...]()` fixtures は 4 バックエンド parity の対象です。

### Collection 検出と heterogeneous semantics (現在の動作)

WASM ランタイムの全 collection detector (`_looks_like_list` / `_is_wasm_set` / `_is_wasm_hashmap` / `_looks_like_pack`) は 8 バイト printable ASCII magic sentinel + 128-bit dual-magic positive identification で統一されています。List / ぶちパック / Set / HashMap の要素 / フィールド / 値スロットに **untagged な大きな Int 値** が入っていても (`@[73088]`, `hashMap().set("x", 73088)`, `setOf(@[73088])` 等)、4 バックエンド全てで byte-for-byte 一致します。`taida_hashmap_set_value_tag` / `taida_list_set_elem_tag` は heterogeneous downgrade に対応し、タグ認識の高速パスを安全化しています。

WASM の tag latching は `WASM_TAG_HETEROGENEOUS = -2` 専用 sentinel に分離されており、型混在コンテナが後続の `.push()` / `.set()` で誤って primitive tag に再昇格しません (`@[1, "a", 2]` / `.set("a", 1).set("b", "x").set("c", 2)` が 4 backend で byte-for-byte 一致)。

Native / WASM の HashMap allocation は末尾に `[next_ord, order_array[cap]]` 挿入順 side-index を持ち、display / entries / keys / values / merge / JSON serialize はすべて insertion 順で walk します (`hashMap().set("a", 1).set("b", 2)` は `"a"` → `"b"` の順で出力)。Interpreter の `Vec<(k,v)>` が source of truth です。

型引数付きの `HashMap[K, V]` / `Set[T]` はメソッド呼び出し後も型引数を保ちます。`HashMap[Str, Int].set(...)` / `.remove(...)` / `.merge(...)` は `HashMap[Str, Int]` を返し、`Set[Int].add(...)` / `.remove(...)` / `.union(...)` / `.intersect(...)` / `.diff(...)` は `Set[Int]` を返します。`HashMap[Str, Int].get(key)` は `Lax[Int]`、`keys()` は `@[Str]`、`values()` は `@[Int]` として扱われます。

### `HashMap.merge(other)` の semantics

`a.merge(b)` は **retain-then-push semantics** に従います: (1) self のうち other に含まれない key のみを self-order で残し、(2) 続けて other の全エントリを other-order で append します。結果として overlap key は **other 側の位置** に移動し、value は other のものになります。

```taida fragment
a = hashMap().set("a", 1).set("b", 2)
b = hashMap().set("c", 3).set("b", 20).set("d", 4)
a.merge(b)  // [a <= 1, c <= 3, b <= 20, d <= 4] (4 backend 一致)
```

### `HashMap.entries()` のフィールド名

`HashMap.entries()` が返すペア pack のフィールド名は **`key` / `value`** に統一されています (`standard_library.md` の `@[@(key, value)]` 仕様と一致)。`zip()` / `Zip[]()` は別仕様で `first` / `second` を使うため、`.entries()` と `zip()` のフィールド名が異なる点に注意してください。

### Bool[x]

値を真偽値に変換し、Lax を返します。

```taida fragment
Bool[1]() ]=> flag       // true
Bool[0]() ]=> flag       // false
```

---

## Lax モールディング型

### Lax[x]

値を Lax で包みます。

```taida
Lax[42]() ]=> val     // 42
Lax[42]().has_value     // true
```

---

## Molten 系統（用途別分岐）

base `Molten` は非 generic の不透明値です。`Molten[T]` や `Molten[Str]` のような型引数つき Molten は導入しません。用途ごとの境界は branch で分けます。

- `Molten => JSON`: 外部由来の raw JSON 境界。Taida 値へ入れる場合は `JSON[raw, Schema]()` を通す。
- `Molten => JS`: JS / npm interop 境界。`Cage` と JS 補助モールドはこの branch だけを受け付ける。
- `Molten => TemperedMolten[T]`: E32 以降の typed boundary 用 branch。E31 では docs lock のみで、JSON / Cage の入力には使わない。

---

## JSON モールディング型（溶鉄）

### JSON[raw, Schema]

生の JSON を型安全な Taida 値へ鋳造します。戻り値は常に `Lax[T]`（パース失敗時は `has_value = false`）。詳細は `docs/guide/03_json.md` を参照してください。

```taida
User = @(name: Str, age: Int)
raw <= '{"name": "Alice", "age": 30}'
JSON[raw, User]() ]=> user
// user: @(name <= "Alice", age <= 30)
```

#### スキーマで受ける型

| スキーマ | 挙動 |
|----------|------|
| プリミティブ (`Int` / `Float` / `Str` / `Bool`) | 型一致なら値、不一致はデフォルト値 |
| クラスライク型（ぶちパック） | フィールド単位で再帰照合 |
| `@[Schema]` | 配列を各要素ごとに再帰照合 |
| **Enum（C16）** | variant 名の Str と照合し ordinal（`Int`）を返す |

#### Enum 検査規則（C16）

- JSON 側の Str が variant 集合に**含まれる** → その variant の ordinal を `Int` として返します。
- 含まれない / キー欠落 / `null` → **`Lax[Enum]`** を返します。`has_value = false`、`__value = __default = Int(0)`（最初のバリアント）。
- silent coercion は**行いません**。利用側は `has_value` / `| .has_value |> ... | _ |> ...` / `getOrDefault(Variant)` で境界を明示処理します（`|==` は throw キャッチ演算子なので Lax には使えません — `docs/reference/operators.md` 参照）。

```taida
Enum => Status = :Active :Inactive :Pending
User = @(name: Str, status: Status)

raw <= '{"name": "Bob", "status": "Bogus"}'
JSON[raw, User]() ]=> u
u.status.has_value                          // false
u.status.getOrDefault(Status:Pending())    // 2
```

`Lax[Enum]` の shape は他の Lax と完全に同一です（`@(has_value, __value, __default, __type="Lax")`）。JSON モールドは 3 バックエンド（Interpreter / JS / Native）で同じ Lax を返します。

---

## Result

### Result[value, predicate]() / Result[value, predicate](throw <= error)

述語付き操作モールドです。`]=>` で述語 P を評価し、真なら値 T を返し、偽なら throw が発動します。

```taida fragment
Mold[T] => Result[T, P <= :T => :Bool] = @(throw: Error)
// P: :T => :Bool（成功条件を定義する述語）

Result[42, _ = true]()                                          // 成功（_ = true は常に真）
Result[0, _ = false](throw <= NotFound(message <= "not found")) // 失敗（_ = false は常に偽）
Result[age, _ x = x >= 18](throw <= err)                        // バリデーション

// => と ]=> の違い
Result[x, pred](throw <= err) => r   // r: Result[T, P]（default solidify = self）
Result[x, pred](throw <= err) ]=> r  // r: T（unmold 時に述語を評価 → 真なら値、偽なら throw）

// 戻り値型注釈: _ で述語を推論
=> :Result[Int, _]
```

---

## Gorillax モールディング型

### Gorillax[x]

値を Gorillax で包みます。unmold 失敗時はゴリラ（プログラム即終了）。Lax とは異なりデフォルト値へのフォールバックはありません。

```taida
Gorillax[42]() ]=> val     // 42
Gorillax[42]().has_value     // true
```

### Cage[subject, runner]

外部由来の Molten 値を扱う boundary。`Cage[subject, runner]()` は subject の Molten branch と runner の `CageRilla[Branch, Out]` 子系統 descriptor を型レベルで照合し、`branch(subject) = B` かつ `runner <: CageRilla[B, Out]` のときだけ branch operation を実行して結果を `Gorillax[Out]` で返します。

```taida fragment
Cage[subject, JSCall[@["sum"], @[items], Int]()]() => result  // result: Gorillax[Int]
// subject : 特定 branch を持つ Molten 値（branch=JS / Build / File）
// runner  : CageRilla[Branch, Out] 子系統 descriptor
// 成功: has_value=true
// 失敗: has_value=false、詳細は errorInfo() で取得
```

subject branch と runner branch の不一致は compile error として弾かれ、`Gorillax(false)` には包まれません。`Gorillax(false)` は branch 一致後の branch operation 失敗だけを表します。Cage 第 2 引数に通常の Taida function / lambda を直接渡す form (旧 `Cage[molten, _ m = ...]()`) は canonical から外れます。

### CageRilla[Branch, Out]

Cage runner の親 type。第 1 引数 `Branch` が runner の branch、第 2 引数 `Out` が成功時 `Gorillax[Out]` の値型を表します。公開 surface の子系統:

| 子系統 | branch | 用途 |
|--------|--------|------|
| `JSRilla[Out]` | JS | npm / JS interop |
| `FileRilla[Out]` | File | file descriptor / handle / cursor / stream |

`JSRilla[JS, Out]` のように branch 名を 2 引数目に書く形は不採用です。子系統 constructor は `[Out]` 1 引数で書きます。

### RelaxedGorillax[T]

`Gorillax.relax()` で生成。unmold 失敗時に `RelaxedGorillaEscaped` エラーを throw します（`|==` で捕捉可能）。

```taida fragment
gorillax.relax() => relaxed  // relaxed: RelaxedGorillax[T]

|== error: RelaxedGorillaEscaped =
  | _ |> defaultValue
=> :T

relaxed ]=> val  // 失敗時: throw（キャッチ可能）
```

### unmold の挙動比較

| 型 | unmold 成功 | unmold 失敗 |
|---|-----------|-----------|
| `Lax[T]` | 値 T を返す | デフォルト値を返す |
| `Gorillax[T]` | 値 T を返す | ゴリラ（プログラム終了） |
| `RelaxedGorillax[T]` | 値 T を返す | `RelaxedGorillaEscaped` を throw |

### errorInfo() による失敗情報取得

`Lax[T]` / `Gorillax[T]` / `RelaxedGorillax[T]` / `RelaxedGorillaEscaped` は失敗時の error 情報を `Lax[ErrorInfo]` として取り出す公式 accessor を備えます。直接 `__error` フィールドへアクセスする旧表記は `[E1960]` で reject されるため、失敗詳細を読みたい場合は `errorInfo()` を使います。詳細を持たない空の `Lax` や成功値に対して呼んだ場合、戻り値の `Lax[ErrorInfo]` も空です。

```taida fragment
result = Cage[subject, JSCall[@["fetch"], @[url], Molten]()]()

// 失敗詳細を読みたい場合
result.errorInfo() ]=> err
err.type        // Str -- error type 名
err.message     // Str -- 人間向けメッセージ
err.kind        // Str -- 細分カテゴリ（"timeout" / "not_found" など）
err.code        // Int -- numeric code（OS error / HTTP status 等）
```

| メソッド | レシーバ | 戻り値 |
|---------|---------|-------|
| `errorInfo()` | `Lax[T]` | `Lax[ErrorInfo]` |
| `errorInfo()` | `Gorillax[T]` | `Lax[ErrorInfo]` |
| `errorInfo()` | `RelaxedGorillax[T]` | `Lax[ErrorInfo]` |
| `errorInfo()` | `Error` 系 (`RelaxedGorillaEscaped` を含む) | `Lax[ErrorInfo]` |

`has_value = true` の値では `errorInfo()` の戻り値 `Lax` の `has_value = false`（実体は無い）。失敗のときだけ、producer が詳細を持っていれば `Lax(true)` で `ErrorInfo` を取り出せます。

### ErrorInfo

失敗 channel 共通の error 情報シェイプ。`errorInfo()` の戻り値に乗ります。

```taida fragment
ErrorInfo = @(
  type: Str        // error type 名（"HttpError" / "IoError" 等）
  message: Str     // 人間向けメッセージ
  kind: Str        // 細分カテゴリ（"timeout" / "not_found" 等。空文字なら未指定）
  code: Int        // numeric code（OS error 番号 / HTTP status / 0 = 未指定）
)
```

各フィールドは default 値を持つため `Lax[ErrorInfo]` の default は `@(type <= "", message <= "", kind <= "", code <= 0)` です。`getOrDefault(...)` で既定値を上書きしながら取り出すこともできます。

---

## JSRilla[Out] 系統 -- JS branch capability constructor（JS バックエンド専用）

`JSRilla[Out]` 系統は JS branch を扱う Cage runner descriptor です。各 constructor は path / args / Out で特定の JS 操作を表現し、実行は `Cage[subject, JSRilla[...]()]()` を介します。

**インタプリタおよび Native バックエンドでは「JS バックエンド専用です」コンパイルエラーになります。**

**3 バックエンド・パリティの対象外です。** これらは JS 連携層に属する機能で、ポータブルなコードでは使いません。インタプリタや Native バックエンドに同等の実装は提供されません。

### JSGet[path, Out]

JS object の property / value を取得します。

```taida
>>> npm:os => @(os)

Cage[os, JSGet[@["platform"], Str]()]() ]=> name  // name: Str
```

| `[]` 必須 | 型 | 説明 |
|----------|-----|------|
| `path` | `@[Str]` | property path。`@[]` で subject 自身、`@["a", "b"]` でネスト |
| `Out` | Type | 取得値の Taida 側型 |

**戻り値**: `JSRilla[Out]`。`Cage` 経由で `Gorillax[Out]`。

### JSCall[path, args, Out]

JS の関数 / メソッドを呼び出します。

```taida
>>> npm:lodash => @(lodash)

items: @[Int] <= @[1, 2, 3, 4, 5]
Cage[lodash, JSCall[@["sum"], @[items], Int]()]() ]=> total  // total: Int = 15
```

| `[]` 必須 | 型 | 説明 |
|----------|-----|------|
| `path` | `@[Str]` | 呼び出し path。`@[]` で subject 自身を呼ぶ |
| `args` | `@[T]` | 引数の list（bridge conversion で JS へ渡る） |
| `Out` | Type | 呼び出し結果の Taida 側型 |

bridge conversion は Taida 値を JS 値へ正規化します。`Lax` は値があれば `__value`、空なら `__default` を渡し、`Result` は `__value` を渡します。`Gorillax` / `RelaxedGorillax` は成功時だけ `__value` を渡し、失敗時は JS の `undefined` になります。この変換は `JSCall` / `JSNew` の `args`、`JSSet` の `value`、`JSSpread` の `source` に適用されます。

**戻り値**: `JSRilla[Out]`。`Cage` 経由で `Gorillax[Out]`。

### JSNew[path, args, Out]

JavaScript の `new` 演算子に相当するコンストラクタ呼び出し。

```taida
>>> npm:express => @(express)

Cage[express, JSNew[@["Router"], @[], Molten]()]() ]=> router         // router: Molten
Cage[express, JSNew[@["Router"], @[@(strict <= true)], Molten]()]() ]=> strictRouter
```

| `[]` 必須 | 型 | 説明 |
|----------|-----|------|
| `path` | `@[Str]` | コンストラクタ path。`@[]` で subject 自身、`@["Router"]` でネスト |
| `args` | `@[T]` | コンストラクタ引数の list |
| `Out` | Type | 生成インスタンスの Taida 側型（多くは `Molten`） |

**戻り値**: `JSRilla[Out]`。`Cage` 経由で `Gorillax[Out]`。

### JSSet[path, value]

JS object のプロパティに値を破壊的に設定します。JavaScript の `subject.path = value` に相当し、**同一 Molten handle を返します**。

```taida fragment
Cage[app, JSSet[@["port"], 3000]()]() ]=> app2     // app2: Molten（app と同一参照）
Cage[config, JSSet[@["debug"], true]()]() ]=> c2   // c2: Molten（config と同一参照）
```

| `[]` 必須 | 型 | 説明 |
|----------|-----|------|
| `path` | `@[Str]` | 設定先 property path。非空必須 |
| `value` | Any | 設定する値（bridge conversion で JS へ渡る） |

**戻り値**: `JSRilla[Molten]` 固定。`Cage` 経由で `Gorillax[Molten]`。

### JSBind[path]

JS object のメソッドに `this` をバインドします。JavaScript の `subject.path.bind(subject)` に相当します。

```taida fragment
Cage[server, JSBind[@["handleRequest"]]()]() ]=> handler  // handler: Molten
Cage[emitter, JSBind[@["emit"]]()]() ]=> callback         // callback: Molten
```

| `[]` 必須 | 型 | 説明 |
|----------|-----|------|
| `path` | `@[Str]` | bind 対象メソッド path |

**戻り値**: `JSRilla[Molten]` 固定。`Cage` 経由で `Gorillax[Molten]`。

### JSSpread[source]

JS object に source の properties をスプレッド展開でマージします。JavaScript の `{...subject, ...source}` に相当します。

```taida fragment
overrides <= @(port <= 8080, debug <= true)
Cage[defaults, JSSpread[overrides]()]() ]=> merged   // merged: Molten
```

| `[]` 必須 | 型 | 説明 |
|----------|-----|------|
| `source` | Any | マージ元の値（bridge conversion で JS へ渡る） |

**戻り値**: `JSRilla[Molten]` 固定。`Cage` 経由で `Gorillax[Molten]`。

---

## パイプラインでの使用

`_` プレースホルダは `[]` 内でも使用可能です。

```taida fragment
// 正方向パイプライン
list => Filter[_, isEven]() => Map[_, _ x = x * 2]() => result

// 逆方向パイプライン（仕様上有効だが、現在のパーサーは <= チェーン未対応。将来対応予定）
// result <= Map[_, _ x = x * 2]() <= Filter[_, isEven]() <= list

// 直接呼び出し（unmold）
Filter[list, isEven]() ]=> result

// 文字列パイプライン
"  Hello!  " => Trim[_]() => Upper[_]() => Split[_, " "]() => result
```

---

## 型シグネチャ一覧

### 文字列モールド

| モールド | `[]` 必須 | `()` オプション | 戻り値 |
|---------|----------|----------------|--------|
| `Upper[str]()` | str | - | Str |
| `Lower[str]()` | str | - | Str |
| `Trim[str]()` | str | start, end | Str |
| `Split[str, delim]()` | str, delim | - | @[Str] |
| `Replace[str, old, new]()` | str, old, new | all | Str |
| `Slice[str]()` | str | start, end | Str |
| `CharAt[str, idx]()` | str, idx | - | Lax[Str] |
| `Repeat[str, n]()` | str, n | - | Str |
| `Reverse[str]()` | str | - | Str |
| `Pad[str, len]()` | str, len | side, char | Str |

### 数値モールド

| モールド | `[]` 必須 | `()` オプション | 戻り値 |
|---------|----------|----------------|--------|
| `ToFixed[num, digits]()` | num, digits | - | Str |
| `Abs[num]()` | num | - | Num |
| `Floor[num]()` | num | - | Int |
| `Ceil[num]()` | num | - | Int |
| `Round[num]()` | num | - | Int |
| `Truncate[num]()` | num | - | Int |
| `Clamp[num, min, max]()` | num, min, max | - | Num |
| `BitAnd[a, b]()` | a, b | - | Int |
| `BitOr[a, b]()` | a, b | - | Int |
| `BitXor[a, b]()` | a, b | - | Int |
| `BitNot[x]()` | x | - | Int |
| `ShiftL[x, n]()` | x, n | - | Lax[Int] |
| `ShiftR[x, n]()` | x, n | - | Lax[Int] |
| `ShiftRU[x, n]()` | x, n | - | Lax[Int] |
| `ToRadix[int, base]()` | int, base | - | Lax[Str] |
| `Int[str, base]()` | str, base | - | Lax[Int] |

### リストモールド

| モールド | `[]` 必須 | `()` オプション | 戻り値 |
|---------|----------|----------------|--------|
| `Reverse[list]()` | list | - | @[T] |
| `Concat[list, other]()` | list, other | - | @[T] / Bytes |
| `Append[list, val]()` | list, val | - | @[T] |
| `Prepend[list, val]()` | list, val | - | @[T] |
| `Join[list, sep]()` | list, sep | - | Str |
| `Sum[list]()` | list | - | Num |
| `Sort[list]()` | list | reverse, by | @[T] |
| `Unique[list]()` | list | by | @[T] |
| `Flatten[list]()` | list | - | @[U] |
| `Find[list, fn]()` | list, fn | - | Lax[T] |
| `FindIndex[list, fn]()` | list, fn | - | Int |
| `Count[list, fn]()` | list, fn | - | Int |
| `Take[list, n]()` | list, n | - | @[T] |
| `Drop[list, n]()` | list, n | - | @[T] |
| `TakeWhile[list, fn]()` | list, fn | - | @[T] |
| `DropWhile[list, fn]()` | list, fn | - | @[T] |
| `Zip[list, other]()` | list, other | - | @[BuchiPack] |
| `Enumerate[list]()` | list | - | @[BuchiPack] |
| `Filter[list, fn]()` | list, fn | - | @[T] |
| `Map[list, fn]()` | list, fn | - | @[U] |
| `Fold[list, init, fn]()` | list, init, fn | - | A |
| `Foldr[list, init, fn]()` | list, init, fn | - | A |

### 条件モールド

| モールド | `[]` 必須 | `()` オプション | 戻り値 |
|---------|----------|----------------|--------|
| `If[cond, then, else]()` | cond, then, else | - | T (then の型) |

### 型比較モールド

| モールド | `[]` 必須 | `()` オプション | 戻り値 |
|---------|----------|----------------|--------|
| `TypeIs[value, :TypeName]()` | value, :TypeName | - | Bool |
| `TypeIs[value, EnumName:Variant]()` | value, EnumName:Variant | - | Bool |
| `TypeExtends[:TypeA, :TypeB]()` | :TypeA, :TypeB | - | Bool |

### 演算・型変換モールド

| モールド | `[]` 必須 | `()` オプション | 戻り値 |
|---------|----------|----------------|--------|
| `Div[x, y]()` | x, y | default | Lax[Num] |
| `Mod[x, y]()` | x, y | - | Lax[Num] |
| `Int[x]()` | x | - | Lax[Int] |
| `Float[x]()` | x | - | Lax[Float] |
| `Str[x]()` | x | - | Lax[Str] |
| `Bool[x]()` | x | - | Lax[Bool] |
| `Ordinal[e]()` | e: Enum | - | Int |
| `UInt8[x]()` | x | - | Lax[Int] |
| `Bytes[x]()` | x | fill | Lax[Bytes] |
| `ByteSet[bytes, idx, value]()` | bytes, idx, value | - | Lax[Bytes] |
| `BytesToList[bytes]()` | bytes | - | @[Int] |
| `Char[x]()` | x | - | Lax[Str] |
| `CodePoint[str]()` | str | - | Lax[Int] |
| `Utf8Encode[str]()` | str | - | Lax[Bytes] |
| `Utf8Decode[bytes]()` | bytes | - | Lax[Str] |
| `Lax[x]()` | x | - | Lax[T] |
| `Gorillax[x]()` | x | - | Gorillax[T] |
| `Cage[subject, runner]()` | subject: Molten, runner: CageRilla[Branch, Out] | - | Gorillax[Out] |
| `TypeName[value]()` | value: T | - | Str |

### JSRilla[Out] 系統（JS バックエンド専用）

| Constructor | `[]` 必須 | `()` オプション | 戻り値 |
|-------------|----------|----------------|--------|
| `JSGet[path, Out]()` | path: @[Str], Out: Type | - | JSRilla[Out] |
| `JSCall[path, args, Out]()` | path: @[Str], args: @[T], Out: Type | - | JSRilla[Out] |
| `JSNew[path, args, Out]()` | path: @[Str], args: @[T], Out: Type | - | JSRilla[Out] |
| `JSSet[path, value]()` | path: @[Str], value: Any | - | JSRilla[Molten] |
| `JSBind[path]()` | path: @[Str] | - | JSRilla[Molten] |
| `JSSpread[source]()` | source: Any | - | JSRilla[Molten] |

---

## E30 過渡期 note (旧 `mold_types.md` からの参照者向け)

このファイル `docs/reference/class_like_types.md` は、E30 (gen-E 破壊的変更) 着手時 (2026-04-28) に **旧 `docs/reference/mold_types.md` から rename** されました。git 履歴は `git log --follow docs/reference/class_like_types.md` で追跡できます。

### 何が変わったか (本ファイル)

- ファイル名: `mold_types.md` → `class_like_types.md`
- タイトル / 概要 (冒頭) を class-like 統一概念向けに再 frame
- Mold 基底クラスの位置づけを「class-like 単一構文を `Mold[...]` 親型で特殊化したもの」として明示
- JSON モールドの schema 表で `TypeDef（ぶちパック）` → `クラスライク型（ぶちパック）` に語彙置換

### 何が変わっていないか

- 標準モールド全種 (文字列 / 数値 / リスト / 演算 / 条件 / 型比較 / 型変換 / Lax / JSON / Result / Gorillax / JS 補助) の API・引数・戻り値・semantics
- `solidify` / `unmold` の正式仕様
- `[]` / `()` 束縛規則
- ヘッダ記法と `[E1401]` / `[E1407]` / `[E1408]` の発火条件（診断コードの意味は `docs/reference/diagnostic_codes.md` と同期して定義されます）
- 4 バックエンド (Interpreter / JS / Native / WASM-wasi) parity 保証

### 旧パスへの参照を見つけた場合

- 本リポジトリ内の非歴史的 docs (README.md / docs/{guide,reference}/ 配下) は一斉置換済み
- `CHANGELOG.md` の **歴史的 release note** (`@b.X` / `@c.X` / `@d.X` 当時の記述) は原文保持 — 当時の事実として `mold_types.md` パスを残しています
- 外部参照 (ブログ / IDE plugin 設定 / addon README 等) で旧パスが残っている場合は、本ファイル `class_like_types.md` を参照するように更新してください

### 関連する変更点

- gen-E でドキュメントを 3 系統 (TypeDef / Mold 継承 / Error 継承) からクラスライク統一概念へ再構成しました。本ファイルへのリネームはその一部です。
- `[E1407]` と `[E1410]` の意味の再定義は、parser と型検査器の実装と同期して反映されています。
