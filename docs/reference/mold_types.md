# モールディング型リファレンス

## 概要

モールディング型の定義方法と、全モールディング型のリファレンスです。
概念的な説明は `../guide/05_molding.md` を参照してください。

> **PHILOSOPHY.md — III.** カタめたいなら、鋳型を作りましょう

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

## Mold基底クラス

すべてのモールディング型は `Mold[...]` を継承して定義します。

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

```taida
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

```taida
Upper["hello"]()              // "HELLO"
str => Upper[_]() => result
```

### Lower[str]

すべての文字を小文字に変換します。

```taida
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

```taida
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

指定位置の文字を返します。Lax[Str] を返し、範囲外の場合は hasValue=false（デフォルト値 ""）。

```taida
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

`n`（シフト量）が `0..63` のとき成功し、`Lax[Int]` を返します。範囲外は `hasValue = false` です。

```taida
ShiftL[1, 40]() ]=> x          // 1099511627776
ShiftRU[-1, 1]() ]=> y         // 9223372036854775807
ShiftL[1, 64]().hasValue       // false
```

### ToRadix[int, base]

整数を指定基数（`2..36`）の文字列へ変換します。

```taida
ToRadix[255, 16]() ]=> s       // "ff"
ToRadix[10, 1]().hasValue      // false
```

### Int[str, base]

指定基数（`2..36`）で文字列を整数に変換します。基数が範囲外の場合は変換失敗になります。符号は先頭 `+` または `-` で表現します。

```taida
Int["ff", 16]() ]=> n          // 255
Int["FF", 16]() ]=> n          // 255 (大文字も受理)
Int["+ff", 16]() ]=> n         // 255 (+ prefix も受理)
Int["1010", 2]() ]=> n         // 10
Int["77", 8]() ]=> n           // 63
Int["-ff", 16]() ]=> n         // -255
Int["2", 2]().hasValue         // false (基数2で "2" は無効)
Int["5", 1]().hasValue         // false (基数1は範囲外)
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
Utf8Decode[badBytes]().hasValue   // false
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

```taida
Sort[@[3, 1, 2]]()                              // @[1, 2, 3]
Sort[@[3, 1, 2]](reverse <= true)               // @[3, 2, 1]
Sort[pilots](by <= _ p = p.syncRate)             // キー関数ソート
Sort[pilots](by <= _ p = p.name, reverse <= true)  // キー関数降順
```

| オプション | デフォルト | 説明 |
|-----------|----------|------|
| `reverse` | `false` | `true` で降順 |
| `by` | なし（自然順） | キー抽出関数 |

### Unique[list]（統合モールド）

重複を除去したリストを返します。

```taida
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
Find[@[1, 2], _ x = x > 10]().hasValue       // false
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

除算を行い、Lax を返します。ゼロ除算の場合は `hasValue = false`。

```taida
Div[10, 3]() ]=> result   // 3
Div[10, 0]() ]=> result   // 0 (ゼロ除算: デフォルト値)
Div[10, 0]().hasValue      // false
```

### Mod[x, y]

剰余を計算し、Lax を返します。ゼロ除算の場合は `hasValue = false`。

```taida
Mod[10, 3]() ]=> result   // 1
Mod[10, 0]().hasValue      // false
```

---

## 条件モールディング型

### If[cond, then, else]

```taida
If[condition, then_value, else_value]() => T
```

2 分岐の条件式。`condition` を評価し、truthy なら `then_value`、falsy なら `else_value` を返します。

- 非選択 branch は評価しません（short-circuit）
- パイプラインで `_` を使って前段の値を参照できます
- ネスト可能: `If[cond, If[cond2, a, b](), c]()`

```taida
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

---

## 型変換モールディング型

型変換モールドが `Lax[...]` を返す挙動は、`solidify` オーバーライドで定義される言語仕様です（専用のコンパイラ特別扱いではない）。

### Int[x]

値を整数に変換し、Lax を返します。文字列から整数への変換（Str → Int）の正規経路です。

```taida
Int["123"]() ]=> num   // 123
Int["abc"]() ]=> num   // 0 (変換失敗: デフォルト値)
Int[3.14]() ]=> num    // 3
Int["+5"]() ]=> num    // 5 (符号付き文字列も受理)
```

受理される文字列: 先頭に `+` または `-` を含むオプションの符号、続いて1桁以上の数字（`0-9`）。空文字列、小数点を含む文字列、先頭/末尾の空白、数字以外の文字を含む文字列は変換失敗になります。

`Int[str, base]()` で基数を指定した変換も可能です（詳細は [数値モールド](#intstr-base) を参照）。

### Float[x]

値を浮動小数点数に変換し、Lax を返します。

```taida
Float["3.14"]() ]=> val  // 3.14
Float[42]() ]=> val      // 42.0
```

### Str[x]

値を文字列に変換し、Lax を返します。`x` はプリミティブ（Int / Float / Bool / Str）だけでなく、List / ぶちパック / 他の Lax など任意の Taida 値を受け取れます。戻り値は常に `Lax[Str]`（失敗条件が無いため `hasValue` は常に `true`）。

```taida
Str[42]() ]=> text       // "42"
Str[3.14]() ]=> text     // "3.14"
Str[3.0]() ]=> text      // "3"       — 整数値のFloatは小数点以下を落とす
Str[-5.0]() ]=> text     // "-5"
Str[true]() ]=> text     // "true"
Str[@[1, 2, 3]]() ]=> t  // "@[1, 2, 3]"
Str[@(a <= 1)]() ]=> t   // "@(a <= 1)"
Str[Int[3.0]()]() ]=> t  // 内側 Lax の full-form 表示:
                          // `@(hasValue <= true, __value <= 3, __default <= 0, __type <= "Lax")`
```

**パリティ保証**: Interpreter / JS / Native / WASM-wasi の 4 バックエンドで同一出力を返します (下記の既知例外 2 件を除く)。Interpreter が参照実装であり、その `Str` の変換規則は以下のとおりです:

- `Int` → 10進表記 (`42` → `"42"`)
- `Float` → Rust `f64::to_string` 相当の最短往復表記（整数値は `.0` を落とす: `3.0` → `"3"`、`3.14` → `"3.14"`）
- `Bool` → `"true"` / `"false"`
- `Str` → そのまま（クォート無し）
- それ以外 (List / ぶちパック / Lax / Result / Async / HashMap / Set / Gorillax / TODO / Molten …) → 各値の display 文字列（ぶちパックは `__` で始まる内部フィールドも含む full-form、nested な HashMap / Set / ぶちパックも再帰的に full-form に展開）

**既知の例外** (`tests/c23_str_parity.rs` でも明示的に skip 指定):

- `Str[Gorillax[v]()]()` の WASM-wasi 出力は、WASM ランタイムが Gorillax の第 1 フィールドを `isOk` で保持している関係で Interpreter / JS / Native の `hasValue` と異なります。

`Stream` 型は 4 バックエンド (Interpreter / JS / Native / WASM-wasi) で `Str[stream]()` の parity を保証します。

### Collection 検出と heterogeneous semantics (現在の動作)

WASM ランタイムの全 collection detector (`_looks_like_list` / `_is_wasm_set` / `_is_wasm_hashmap` / `_looks_like_pack`) は 8 バイト printable ASCII magic sentinel + 128-bit dual-magic positive identification で統一されています。List / ぶちパック / Set / HashMap の要素 / フィールド / 値スロットに **untagged な大きな Int 値** が入っていても (`@[73088]`, `hashMap().set("x", 73088)`, `setOf(@[73088])` 等)、4 バックエンド全てで byte-for-byte 一致します。`taida_hashmap_set_value_tag` / `taida_list_set_elem_tag` は heterogeneous downgrade に対応し、タグ認識の高速パスを安全化しています。

WASM の tag latching は `WASM_TAG_HETEROGENEOUS = -2` 専用 sentinel に分離されており、型混在コンテナが後続の `.push()` / `.set()` で誤って primitive tag に再昇格しません (`@[1, "a", 2]` / `.set("a", 1).set("b", "x").set("c", 2)` が 4 backend で byte-for-byte 一致)。

Native / WASM の HashMap allocation は末尾に `[next_ord, order_array[cap]]` 挿入順 side-index を持ち、display / entries / keys / values / merge / JSON serialize はすべて insertion 順で walk します (`hashMap().set("a", 1).set("b", 2)` は `"a"` → `"b"` の順で出力)。Interpreter の `Vec<(k,v)>` が source of truth です。

### `HashMap.merge(other)` の semantics

`a.merge(b)` は **retain-then-push semantics** に従います: (1) self のうち other に含まれない key のみを self-order で残し、(2) 続けて other の全エントリを other-order で append します。結果として overlap key は **other 側の位置** に移動し、value は other のものになります。

```taida
a = hashMap().set("a", 1).set("b", 2)
b = hashMap().set("c", 3).set("b", 20).set("d", 4)
a.merge(b)  // [a <= 1, c <= 3, b <= 20, d <= 4] (4 backend 一致)
```

### `HashMap.entries()` のフィールド名

`HashMap.entries()` が返すペア pack のフィールド名は **`key` / `value`** に統一されています (`standard_library.md` の `@[@(key, value)]` 仕様と一致)。`zip()` / `Zip[]()` は別仕様で `first` / `second` を使うため、`.entries()` と `zip()` のフィールド名が異なる点に注意してください。

### Bool[x]

値を真偽値に変換し、Lax を返します。

```taida
Bool[1]() ]=> flag       // true
Bool[0]() ]=> flag       // false
```

---

## Lax モールディング型

### Lax[x]

値を Lax で包みます。

```taida
Lax[42]() ]=> val     // 42
Lax[42]().hasValue     // true
```

---

## JSON モールディング型（溶鉄）

### JSON[raw, Schema]

生の JSON を型安全な Taida 値へ鋳造します。戻り値は常に `Lax[T]`（パース失敗時は `hasValue = false`）。詳細は `docs/guide/03_json.md` を参照してください。

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
| TypeDef（ぶちパック） | フィールド単位で再帰照合 |
| `@[Schema]` | 配列を各要素ごとに再帰照合 |
| **Enum（C16）** | variant 名の Str と照合し ordinal（`Int`）を返す |

#### Enum 検査規則（C16）

- JSON 側の Str が variant 集合に**含まれる** → その variant の ordinal を `Int` として返します。
- 含まれない / キー欠落 / `null` → **`Lax[Enum]`** を返します。`hasValue = false`、`__value = __default = Int(0)`（最初のバリアント）。
- silent coercion は**行いません**。利用側は `hasValue` / `| .hasValue |> ... | _ |> ...` / `getOrDefault(Variant)` で境界を明示処理します（`|==` は throw キャッチ演算子なので Lax には使えません — `docs/reference/operators.md` 参照）。

```taida
Enum => Status = :Active :Inactive :Pending
User = @(name: Str, status: Status)

raw <= '{"name": "Bob", "status": "Bogus"}'
JSON[raw, User]() ]=> u
u.status.hasValue                          // false
u.status.getOrDefault(Status:Pending())    // 2
```

`Lax[Enum]` の shape は他の Lax と完全に同一です（`@(hasValue, __value, __default, __type="Lax")`）。JSON モールドは 3 バックエンド（Interpreter / JS / Native）で同じ Lax を返します。

---

## Result

### Result[value, predicate]() / Result[value, predicate](throw <= error)

述語付き操作モールドです。`]=>` で述語 P を評価し、真なら値 T を返し、偽なら throw が発動します。

```taida
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
Gorillax[42]().hasValue     // true
```

### Cage[molten, function]

Molten（溶鉄）に対して関数を実行し、結果を Gorillax で返します。関数の実行が失敗した場合は `hasValue = false` の Gorillax が返ります。

**Cage は Molten 専用です。** 第一引数は Molten 型の値のみ受け付けます。

```taida
Cage[molten, _ x = x.someMethod()] => result  // result: Gorillax[U]
// molten: Molten（溶鉄のみ）
// 成功: Gorillax(hasValue=true, __value=結果)
// 失敗: Gorillax(hasValue=false, __error=エラー)
```

### RelaxedGorillax[T]

`Gorillax.relax()` で生成。unmold 失敗時に `RelaxedGorillaEscaped` エラーを throw します（`|==` で捕捉可能）。

```taida
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

---

## JS 補助モールド（JS バックエンド専用）

npm パッケージから得られる Molten 値に対して JavaScript 固有の操作を行うモールドです。全て Molten を受け取り Molten を返します。

**インタプリタおよび Native バックエンドでは「JS バックエンド専用です」コンパイルエラーになります。**

**3バックエンド・パリティの対象外です。** これらは JS interop 層であり、ポータブルなコードでは使いません。インタプリタや Native バックエンドに同等の実装は提供されません。

### JSNew[constructor, args]

JavaScript の `new` 演算子に相当するコンストラクタ呼び出しです。

```taida
>>> npm:express => @(express)

JSNew[express.Router, @()]() => router  // router: Molten
JSNew[express.Router, @(strict <= true)]() => strictRouter  // strictRouter: Molten
```

| `[]` 必須 | 型 | 説明 |
|----------|-----|------|
| `constructor` | Molten | コンストラクタ関数 |
| `args` | BuchiPack | コンストラクタ引数 |

**戻り値**: `Molten`

### JSSet[obj, key, value]

Molten オブジェクトのプロパティに値を破壊的に設定します。JavaScript の `obj[key] = value` に相当します。**同一の Molten 参照を返します** -- Molten の世界では JavaScript の破壊的代入のセマンティクスがそのまま適用されます。

```taida
JSSet[app, "port", 3000]() => app2    // app2: Molten（app と同一参照）
JSSet[config, "debug", true]() => c2  // c2: Molten（config と同一参照）
```

| `[]` 必須 | 型 | 説明 |
|----------|-----|------|
| `obj` | Molten | 対象オブジェクト |
| `key` | Str | プロパティ名 |
| `value` | Any | 設定する値 |

**戻り値**: `Molten`（同一参照。JavaScript の破壊的代入に相当）

### JSBind[obj, method]

Molten オブジェクトのメソッドに `this` をバインドします。JavaScript の `obj.method.bind(obj)` に相当します。

```taida
handler <= JSBind[server, "handleRequest"]()  // handler: Molten
callback <= JSBind[emitter, "emit"]()         // callback: Molten
```

| `[]` 必須 | 型 | 説明 |
|----------|-----|------|
| `obj` | Molten | 対象オブジェクト |
| `method` | Str | メソッド名 |

**戻り値**: `Molten`

### JSSpread[target, source]

Molten オブジェクトにプロパティをスプレッド展開でマージします。JavaScript の `{...target, ...source}` に相当します。

```taida
overrides <= @(port <= 8080, debug <= true)
merged <= JSSpread[defaults, overrides]()  // merged: Molten
```

| `[]` 必須 | 型 | 説明 |
|----------|-----|------|
| `target` | Molten | マージ先オブジェクト |
| `source` | Any | マージ元の値 |

**戻り値**: `Molten`

---

## パイプラインでの使用

`_` プレースホルダは `[]` 内でも使用可能です。

```taida
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
| `Cage[molten, fn]()` | molten: Molten, fn | - | Gorillax[U] |

### JS 補助モールド（JS バックエンド専用）

| モールド | `[]` 必須 | `()` オプション | 戻り値 |
|---------|----------|----------------|--------|
| `JSNew[constructor, args]()` | constructor: Molten, args: BuchiPack | - | Molten |
| `JSSet[obj, key, value]()` | obj: Molten, key: Str, value: Any | - | Molten |
| `JSBind[obj, method]()` | obj: Molten, method: Str | - | Molten |
| `JSSpread[target, source]()` | target: Molten, source: Any | - | Molten |
