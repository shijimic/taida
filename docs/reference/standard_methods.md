# 標準メソッドリファレンス

## 概要

プリミティブ型およびリスト型で使用できる標準メソッドのリファレンスです。

メソッドは**状態チェック**（内部状態を問い合わせる）と**表示用**（toString）に限定されています。
操作系の処理はモールドとして提供されます。詳細は `reference/class_like_types.md` を参照してください。

## 設計原則

- **状態チェックメソッド**: 対象の内部状態を Bool/Int で返す。変換は行わない
- **モナディック操作メソッド**: Result/Lax/Async 固有の map/flatMap/mapError
- **toString メソッド**: デバッグ・表示用の文字列変換
- **操作はモールドで**: 値の変換・加工はモールドを使用する

> **PHILOSOPHY.md — III.** カタめたいなら、鋳型を作りましょう

---

## `.toString()` — 全型共通の表示メソッド

全ての値は `.toString()` を呼び出せます。戻り値は常に `:Str` で、
インタプリタ / JS / Native の 3 バックエンドで同一の文字列が得られます。

```taida
42.toString()             // "42"
3.14.toString()           // "3.14"
true.toString()           // "true"
"hello".toString()        // "hello"
@[1, 2, 3].toString()     // "@[1, 2, 3]"
@(a <= 1, b <= 2).toString()  // "@(a <= 1, b <= 2)"
```

- 引数なしで呼び出すこと。`n.toString(16)` のように base や precision を
  渡そうとすると、チェッカーが `[E1508] Method 'toString' takes 0 argument(s)`
  で拒否します（哲学 I: 暗黙の型変換なし）。
- 基数指定で整数を文字列化したい場合は `ToRadix[n, base]()` モールド
  （`reference/class_like_types.md §ToRadix`）を使います。戻り値は `Lax[Str]`
  なので `.getOrDefault("")` で unwrap します。
- 数値を Lax にラップして扱いたい場合は `Str[value]()` モールドを使います。
  `.toString()` は直接 `Str` を返すため、`+` 演算子で文字列連結する場合に
  便利です。

---

## Str — 文字列メソッド

### 状態チェック

#### length

文字列の長さを返します。

```taida
"hello".length()  // 5
"".length()       // 0
"日本語".length()   // 3 (文字数)
```

**シグネチャ**: `=> :Int`

#### contains

部分文字列が含まれているかを返します。

```taida
"hello world".contains("world")  // true
"hello".contains("xyz")  // false
```

**シグネチャ**: `substr: Str => :Bool`

#### startsWith

指定した文字列で始まるかを返します。

```taida
"hello world".startsWith("hello")  // true
"hello".startsWith("world")  // false
```

**シグネチャ**: `prefix: Str => :Bool`

#### endsWith

指定した文字列で終わるかを返します。

```taida
"hello world".endsWith("world")  // true
"hello".endsWith("xyz")  // false
```

**シグネチャ**: `suffix: Str => :Bool`

#### indexOf

部分文字列の位置を返します。見つからない場合は -1。

```taida
"hello world".indexOf("world")  // 6
"hello".indexOf("xyz")  // -1
```

**シグネチャ**: `substr: Str => :Int`

#### lastIndexOf

部分文字列の最後の出現位置を返します。見つからない場合は -1。

```taida
"hello hello".lastIndexOf("hello")  // 6
"hello".lastIndexOf("xyz")  // -1
```

**シグネチャ**: `substr: Str => :Int`

### 安全アクセス

#### get

指定インデックスの文字を Lax で返します。範囲外の場合は `hasValue = false`。

```taida
"hello".get(0) ]=> ch    // "h"
"hello".get(10) ]=> ch   // "" (デフォルト値)
"hello".get(10).hasValue  // false
```

**シグネチャ**: `index: Int => :Lax[Str]`

### 文字列変換

#### toString

文字列をそのまま返します（他の型との一貫性のため）。

```taida
"hello".toString()  // "hello"
```

**シグネチャ**: `=> :Str`

### 文字列操作

#### replace

最初に一致した部分文字列を置換します。引数が `:Str` の場合は固定文字列
マッチ、`:Regex` の場合は正規表現マッチになります。

```taida
"hello world".replace("world", "taida")  // "hello taida"
"aaa".replace("a", "b")  // "baa" (最初の一致のみ)

// C12 Phase 6 (FB-5): Regex overload
"hello".replace(Regex("[aeiou]"), "*")  // "h*llo"
```

**シグネチャ**:

- `target: Str, replacement: Str => :Str`
- `pattern: Regex, replacement: Str => :Str`

空ターゲット文字列の場合は元の文字列を返します（no-op）。Regex 置換で
は JS の `$&` / `$1` 系メタ構文は無効化され、置換文字列はリテラルとして
挿入されます。

#### replaceAll

すべての一致する部分文字列を置換します。

```taida
"hello world hello".replaceAll("hello", "hi")  // "hi world hi"
"aaa".replaceAll("a", "b")  // "bbb"

// C12 Phase 6 (FB-5): Regex overload
"hello".replaceAll(Regex("[aeiou]"), "*")  // "h*ll*"
"a1b2c3".replaceAll(Regex("\\d"), "#")     // "a#b#c#"
```

**シグネチャ**:

- `target: Str, replacement: Str => :Str`
- `pattern: Regex, replacement: Str => :Str`

空ターゲット文字列の場合は元の文字列を返します（no-op）。

#### split

文字列をセパレータで分割し、リストを返します。

```taida
"a,b,c".split(",")  // @["a", "b", "c"]
"hello".split(",")  // @["hello"] (一致なし → 単一要素)
"abc".split("")     // @["a", "b", "c"] (文字ごとに分割)

// C12 Phase 6 (FB-5): Regex overload
"one,two;three.four".split(Regex("[,;.]"))  // @["one", "two", "three", "four"]
```

**シグネチャ**:

- `separator: Str => :List[Str]`
- `pattern: Regex => :List[Str]`

空セパレータの場合は各文字に分割します（`Chars[]` モールドと同等）。

#### match

正規表現で最初の一致を取り出します。結果は `:RegexMatch` ぶちパックで、
`hasValue`, `full`, `groups`, `start` フィールドを持ちます。

```taida
m <= "id: 12-34".match(Regex("(\\d+)-(\\d+)"))
stdout(m.hasValue)  // true
stdout(m.full)       // "12-34"
stdout(m.groups)     // @["12", "34"]
stdout(m.start)      // 4  (char index, not byte index)
```

**シグネチャ**: `pattern: Regex => :RegexMatch`

一致なしの場合 `hasValue <= false`, `full <= ""`, `groups <= @[]`,
`start <= -1` のぶちパックを返します（null / undefined は返さない —
哲学 I）。

#### search

正規表現で最初の一致が見つかった位置（char index）を返します。一致が
ない場合は `-1` を返します。

```taida
stdout("abc123".search(Regex("\\d+")))  // 3
stdout("nothing".search(Regex("\\d+"))) // -1
```

**シグネチャ**: `pattern: Regex => :Int`

文字列内での固定文字列検索には `.indexOf(...)` を使用してください。
`search` は必ず `Regex` 引数を要求します。

#### Regex コンストラクタ

`Regex(pattern, flags?)` で正規表現オブジェクトを作成します（C12 Phase 6）。

```taida
r <= Regex("[aeiou]")          // flags なし
ri <= Regex("hello", "i")      // case-insensitive
```

**シグネチャ**: `pattern: Str, flags: Str => :Regex`

サポートされるフラグ:

| フラグ | 意味 |
|--------|------|
| `i` | 大文字小文字を無視 |
| `m` | 複数行モード（`^` / `$` が各行にマッチ） |
| `s` | dotall — `.` が改行にマッチ（Interpreter / JS のみ。Native の POSIX ERE はサポート外）|

サポートされるエスケープ:

| エスケープ | 意味 | 3 backend 対応 |
|-----------|------|----------------|
| `\d` / `\D` | 数字 / 数字以外 | 全 backend |
| `\w` / `\W` | 単語文字（英数字 + `_`）/ それ以外 | 全 backend |
| `\s` / `\S` | 空白文字 / 空白以外 | 全 backend |
| `\xHH` / `\x{HH…}` | 16 進コードポイント（1 バイト）| 全 backend |
| `\uHHHH` / `\u{HH…}` | Unicode コードポイント（UTF-8 エンコード）| 全 backend |
| `\\` | リテラルな `\` | 全 backend |
| `\b` / `\B` | 単語境界 / 単語境界以外 | Interpreter / JS のみ |

Native POSIX ERE は `\b` / `\B` に対応する概念を持ちません。単語境界が
必要な場合は Interpreter / JS ターゲットに限定するか、
`(^|[^A-Za-z0-9_])` などで明示的に囲むパターンに書き換えてください。

上記の表にない `\_` / `\/` のような意味を持たない「identity escape」は、
Interpreter（Rust `regex`）と Native（POSIX ERE）が lenient に受け付ける
ため、JS backend でも同じ挙動になるようコンパイル時に JS の `/u` 厳格
モードを無効化しています。

`\x{HH…}` / `\u{HH…}` の bracket 形式は JS の `/u` モードでしか native に
解釈されませんが、同じ理由で `/u` を使えないため、JS ランタイムは
bracket 形式を `\uHHHH` あるいは UTF-16 サロゲートペアへ事前 rewrite
してから `RegExp` へ渡します。結果として **construct 時の検証と
first-use 時の実行が同じ文法**で動作し、`Regex("\x{41}")` のような
パターンが構築だけ成功して初回使用時に throw するといった asymmetric
な失敗モードは発生しません。

不正なフラグや不正なパターンは **3 backend 全て** で構築時に
`:Error`（`ValueError`）が投げられます。JavaScript の
`$&` / `$1` 等の置換メタ構文は無効化されており、置換文字列はリテラルと
して挿入されます。

### 操作はモールドでも可能

以下の操作はモールドとしても提供されます:

| 操作 | モールド | 例 |
|------|---------|-----|
| 大文字変換 | `Upper[str]()` | `Upper["hello"]()` → `"HELLO"` |
| 小文字変換 | `Lower[str]()` | `Lower["HELLO"]()` → `"hello"` |
| 空白除去 | `Trim[str]()` | `Trim["  hi  "]()` → `"hi"` |
| 分割 | `Split[str, delim]()` | `Split["a,b", ","]()` → `@["a", "b"]` |
| 置換 | `Replace[str, old, new]()` | `Replace["ab", "a", "x"]()` → `"xb"` |
| 範囲抽出 | `Slice[str]()` | `Slice["hello"](end <= 3)` → `"hel"` |
| 文字取得 | `CharAt[str, idx]()` | `CharAt["hello", 0]() ]=> ch` → `"h"` (Lax[Str]) |
| 繰り返し | `Repeat[str, n]()` | `Repeat["ha", 3]()` → `"hahaha"` |
| 逆順 | `Reverse[str]()` | `Reverse["hello"]()` → `"olleh"` |
| パディング | `Pad[str, len]()` | `Pad["42", 5](side <= "start", char <= "0")` → `"00042"` |
| 型変換 | `Int[str]()` / `Int[str, base]()` / `Float[str]()` | `Int["42"]() ]=> num` → `42`, `Int["ff", 16]() ]=> hex` → `255` |

---

## Bytes — バイト列メソッド

`Bytes` は 0..255 の連続領域です。バイナリ境界で使います。

### 状態チェック

#### length

バイト数を返します。

```taida
Bytes["ping"]() ]=> b
b.length()  // 4
```

**シグネチャ**: `=> :Int`

### 安全アクセス

#### get

指定インデックスのバイト値を `Lax[Int]` で返します。範囲外は `hasValue = false`。

```taida
Bytes[@[65, 66]]() ]=> b
b.get(0) ]=> v    // 65
b.get(99).hasValue  // false
```

**シグネチャ**: `index: Int => :Lax[Int]`

### 文字列変換

#### toString

表示用の `Bytes[@[...]]` 形式へ変換します。

```taida
Bytes[@[65, 66]]() ]=> b
b.toString()  // "Bytes[@[65, 66]]"
```

**シグネチャ**: `=> :Str`

### 操作はモールドで

```taida
ByteSet[b, 1, 67]() ]=> b2
Slice[b](start <= 0, end <= 1)      // Bytes
Concat[b, b2]()                      // Bytes
BytesToList[b2]()                    // @[Int]
Utf8Decode[b2]()                     // Lax[Str]
```

---

## Molten -- 溶鉄（メソッドなし）

`Molten` 型はメソッドを一切持ちません。Molten は外部由来の不透明値であり、直接操作はできません。型パラメータはありません -- Molten は Molten でしかありません。

```taida
>>> npm:lodash => @(lodash)  // lodash: Molten

// これらは全てエラーになります
lodash.sum()          // エラー: Molten has no methods
lodash.toString()     // エラー: Molten has no methods
lodash.length()       // エラー: Molten has no methods
```

JS / npm 連携の Molten 値（`Molten => JS`）を操作するには Cage を使います。JS バックエンドでは JSNew, JSSet, JSBind, JSSpread モールドも利用できます。

JSON は `Molten => JSON` の分岐です。JSON もメソッドを持ちません（詳細は下記の JSON セクションを参照）。

---

## JSON -- 溶鉄（メソッドなし）

`JSON` 型はメソッドを一切持ちません。JSON は `Molten => JSON` の分岐であり、外部由来の不透明値です。`Molten[Str]` のような型引数つきの Molten でもなく、Cage で扱う JS 分岐でもありません。

```taida
// これらは全てエラーになります
data.at("name")       // エラー: JSON has no methods
data.toStr()           // エラー: JSON has no methods
data.keys()            // エラー: JSON has no methods
data ]=> x             // エラー: JSON direct unmold is not allowed
```

JSON を使うには `JSON[raw, Schema]()` でスキーマを指定してください。詳細は [JSON 溶鉄](../guide/03_json.md) を参照してください。

---

## Num — 数値メソッド (Int / Float)

### 状態チェック

#### isNaN

NaN (非数) かどうかを返します。

```taida
42.isNaN()     // false
0.0.isNaN()    // false
```

**シグネチャ**: `=> :Bool`

> **注意**: Taida に `/` 演算子はなく、`Div[x, y]()` モールドが Lax を返すため、通常の計算で NaN が生成されることはありません。外部データ（JSON など）から受け取った値の検査に使用します。

#### isInfinite

無限大かどうかを返します。

```taida
42.isInfinite()  // false
```

**シグネチャ**: `=> :Bool`

#### isFinite

有限数かどうかを返します。

```taida
42.isFinite()   // true
3.14.isFinite() // true
```

**シグネチャ**: `=> :Bool`

#### isPositive

正の数かどうかを返します。

```taida
5.isPositive()  // true
(-5).isPositive()  // false
0.isPositive()  // false
```

**シグネチャ**: `=> :Bool`

#### isNegative

負の数かどうかを返します。

```taida
(-5).isNegative()  // true
5.isNegative()  // false
```

**シグネチャ**: `=> :Bool`

#### isZero

ゼロかどうかを返します。

```taida
0.isZero()  // true
0.0.isZero()  // true
1.isZero()  // false
```

**シグネチャ**: `=> :Bool`

### 文字列変換

#### toString

数値を文字列に変換します。

```taida
42.toString()  // "42"
3.14.toString()  // "3.14"
```

**シグネチャ**: `=> :Str`

### 操作はモールドで

以下の操作はモールドとして提供されます:

| 操作 | モールド | 例 |
|------|---------|-----|
| 小数点固定 | `ToFixed[num, digits]()` | `ToFixed[3.14159, 2]()` → `"3.14"` |
| 絶対値 | `Abs[num]()` | `Abs[-5]()` → `5` |
| 切り捨て | `Floor[num]()` | `Floor[3.7]()` → `3` |
| 切り上げ | `Ceil[num]()` | `Ceil[3.2]()` → `4` |
| 四捨五入 | `Round[num]()` | `Round[3.5]()` → `4` |
| 0方向切り捨て | `Truncate[num]()` | `Truncate[3.7]()` → `3` |
| 範囲制限 | `Clamp[num, min, max]()` | `Clamp[15, 0, 10]()` → `10` |

---

## List — リストメソッド

### 状態チェック

#### length

リストの要素数を返します。

```taida
@[1, 2, 3].length()  // 3
@[].length()  // 0
```

**シグネチャ**: `=> :Int`

#### isEmpty

リストが空かどうかを返します。

```taida
@[].isEmpty()  // true
@[1].isEmpty()  // false
```

**シグネチャ**: `=> :Bool`

#### contains

要素が含まれているかを返します。

```taida
@[1, 2, 3].contains(2)  // true
@[1, 2, 3].contains(5)  // false
```

**シグネチャ**: `item: T => :Bool`

#### indexOf

要素の位置を返します。見つからない場合は -1。

```taida
@[10, 20, 30].indexOf(20)  // 1
@[10, 20, 30].indexOf(50)  // -1
```

**シグネチャ**: `item: T => :Int`

#### lastIndexOf

要素の最後の出現位置を返します。見つからない場合は -1。

```taida
@[1, 2, 1, 2].lastIndexOf(1)  // 2
```

**シグネチャ**: `item: T => :Int`

### 安全アクセス（Lax 返し）

#### first

最初の要素を Lax で返します。空リストの場合は `hasValue = false`。

```taida
@[1, 2, 3].first() ]=> val  // 1
@[].first().hasValue         // false
```

**シグネチャ**: `=> :Lax[T]`

#### last

最後の要素を Lax で返します。空リストの場合は `hasValue = false`。

```taida
@[1, 2, 3].last() ]=> val  // 3
@[].last().hasValue         // false
```

**シグネチャ**: `=> :Lax[T]`

#### get

指定インデックスの要素を Lax で返します。範囲外の場合は `hasValue = false`。

```taida
@[10, 20, 30].get(1) ]=> val    // 20
@[10, 20, 30].get(10).hasValue  // false
```

**シグネチャ**: `index: Int => :Lax[T]`

#### max

最大値を Lax で返します。空リストの場合は `hasValue = false`。

```taida
@[1, 3, 2].max() ]=> val  // 3
@[].max().hasValue         // false
```

**シグネチャ**: `=> :Lax[T]`

#### min

最小値を Lax で返します。空リストの場合は `hasValue = false`。

```taida
@[1, 3, 2].min() ]=> val  // 1
@[].min().hasValue         // false
```

**シグネチャ**: `=> :Lax[T]`

### 述語（状態チェック）

#### any

いずれかの要素が条件を満たすかを返します。

```taida
@[1, 2, 3].any(_ x = x > 2)  // true
@[1, 2, 3].any(_ x = x > 10)  // false
```

**シグネチャ**: `pred: :T => :Bool => :Bool`

#### all

すべての要素が条件を満たすかを返します。

```taida
@[2, 4, 6].all(_ x = Mod[x, 2]().unmold() == 0)  // true
@[1, 2, 3].all(_ x = x > 2)  // false
```

**シグネチャ**: `pred: :T => :Bool => :Bool`

#### none

すべての要素が条件を満たさないかを返します。

```taida
@[1, 2, 3].none(_ x = x > 10)  // true
@[1, 2, 3].none(_ x = x > 2)  // false
```

**シグネチャ**: `pred: :T => :Bool => :Bool`

### 操作はモールドで

以下の操作はモールドとして提供されます:

| 操作 | モールド | 例 |
|------|---------|-----|
| 逆順 | `Reverse[list]()` | `Reverse[@[1,2,3]]()` → `@[3,2,1]` |
| 結合 | `Concat[list, other]()` | `Concat[@[1,2], @[3,4]]()` → `@[1,2,3,4]` |
| 末尾追加 | `Append[list, val]()` | `Append[@[1,2], 3]()` → `@[1,2,3]` |
| 先頭追加 | `Prepend[list, val]()` | `Prepend[@[2,3], 1]()` → `@[1,2,3]` |
| 文字列結合 | `Join[list, sep]()` | `Join[@["a","b"], ","]()` → `"a,b"` |
| 合計 | `Sum[list]()` | `Sum[@[1,2,3]]()` → `6` |
| ソート | `Sort[list]()` | `Sort[@[3,1,2]]()` → `@[1,2,3]` |
| 重複除去 | `Unique[list]()` | `Unique[@[1,2,2,3]]()` → `@[1,2,3]` |
| フラット化 | `Flatten[list]()` | `Flatten[@[@[1],@[2]]]()` → `@[1,2]` |
| 条件検索 | `Find[list, fn]()` | `Find[@[1,2,3], _ x = x > 1]()` → `Lax(2)` |
| 位置検索 | `FindIndex[list, fn]()` | `FindIndex[@[1,2,3], _ x = x > 1]()` → `1` |
| 条件カウント | `Count[list, fn]()` | `Count[@[1,2,3], _ x = x > 1]()` → `2` |
| 先頭n個 | `Take[list, n]()` | `Take[@[1,2,3], 2]()` → `@[1,2]` |
| スキップ | `Drop[list, n]()` | `Drop[@[1,2,3], 1]()` → `@[2,3]` |
| ペア化 | `Zip[list, other]()` | `Zip[@[1,2], @["a","b"]]()` |
| インデックス付与 | `Enumerate[list]()` | `Enumerate[@["a","b"]]()` |
| フィルタ | `Filter[list, fn]()` | `Filter[@[1,2,3], _ x = x > 1]()` → `@[2,3]` |
| 変換 | `Map[list, fn]()` | `Map[@[1,2,3], _ x = x * 2]()` → `@[2,4,6]` |
| 左畳み込み | `Fold[list, init, fn]()` | `Fold[@[1,2,3], 0, _ a x = a + x]()` → `6` |
| 右畳み込み | `Foldr[list, init, fn]()` | `Foldr[@[1,2,3], 0, _ a x = a + x]()` → `6` |

---

## Bool — ブールメソッド

### 文字列変換

#### toString

真偽値を文字列に変換します。

```taida
true.toString()  // "true"
false.toString()  // "false"
```

**シグネチャ**: `=> :Str`

### 型変換はモールドで

`Int[bool]()` を使用してください。

```taida
Int[true]() ]=> num   // 1
Int[false]() ]=> num  // 0
```

---

## Lax — Lax メソッド

`Lax[T]` 型に対して使用できるメソッドです。Lax は `Div[x, y]()`、`Mod[x, y]()`、`get()`、`first()`、`last()`、`max()`、`min()` などの戻り値として得られます。

### フィールド

#### hasValue

値を持つかどうかを示すブールフィールドです。

```taida
Div[10, 3]().hasValue   // true
Div[10, 0]().hasValue   // false
```

**型**: `Bool`

### 検査

#### isEmpty

値を持たないかどうかを返します。`!hasValue` と同じです。

```taida
Div[10, 0]().isEmpty()  // true
Div[10, 3]().isEmpty()  // false
```

**シグネチャ**: `=> :Bool`

### 値の取得

#### getOrDefault

値があればその値を、なければ指定したデフォルト値を返します。

```taida
Div[10, 3]().getOrDefault(99)  // 3
Div[10, 0]().getOrDefault(99)  // 99
```

**シグネチャ**: `default: T => :T`

#### unmold

値を取り出します。`hasValue = false` の場合は型 T のデフォルト値を返します。

```taida
Div[10, 3]().unmold()  // 3
Div[10, 0]().unmold()  // 0 (Int のデフォルト値)
```

**シグネチャ**: `=> :T`

### モナディック操作

#### map

値がある場合に関数を適用し、新しい Lax を返します。

```taida
Div[10, 2]().map(_ x = x * 3) ]=> result  // 15
Div[10, 0]().map(_ x = x * 3) ]=> result  // 0 (空のまま)
```

**シグネチャ**: `fn: :T => :U => :Lax[U]`

#### flatMap

値がある場合に関数を適用し、関数が返す Lax をそのまま返します。

```taida
Div[10, 2]().flatMap(_ x = Div[x, 3]()) ]=> result  // 1
```

**シグネチャ**: `fn: :T => :Lax[U] => :Lax[U]`

### 文字列変換

#### toString

Lax の文字列表現を返します。

```taida
Div[10, 3]().toString()  // "Lax(3)"
Div[10, 0]().toString()  // "Lax(default: 0)"
```

**シグネチャ**: `=> :Str`

---

## Gorillax — 覚悟のモールドメソッド

`Gorillax[T]` 型に対して使用できるメソッドです。Gorillax は `Cage[value, fn]()` や `Gorillax[value]()` で生成されます。

### フィールド

#### hasValue

値を持つかどうかを示すブールフィールドです。

```taida
Gorillax[42]().hasValue  // true
```

**型**: `Bool`

### 検査

#### isEmpty

値を持たないかどうかを返します。

```taida
Gorillax[42]().isEmpty()  // false
```

**シグネチャ**: `=> :Bool`

### 変換

#### relax

Gorillax を RelaxedGorillax に変換します。unmold 失敗時のゴリラ（即終了）が `RelaxedGorillaEscaped` エラーの throw に変わり、`|==` でキャッチ可能になります。

```taida
Gorillax[42]().relax()  // RelaxedGorillax(42)
```

**シグネチャ**: `=> :RelaxedGorillax[T]`

### 文字列変換

#### toString

Gorillax の文字列表現を返します。

```taida
Gorillax[42]().toString()  // "Gorillax(42)"
```

**シグネチャ**: `=> :Str`

---

## RelaxedGorillax — リラックスしたゴリラメソッド

`RelaxedGorillax[T]` 型に対して使用できるメソッドです。`Gorillax.relax()` で生成されます。

### フィールド

#### hasValue

値を持つかどうかを示すブールフィールドです。

**型**: `Bool`

### 検査

#### isEmpty

値を持たないかどうかを返します。

**シグネチャ**: `=> :Bool`

### 文字列変換

#### toString

RelaxedGorillax の文字列表現を返します。

```taida
gorillax.relax().toString()  // "RelaxedGorillax(42)"
```

**シグネチャ**: `=> :Str`

---

## Result — モナディック型メソッド

`Result[T, P]` は `Mold[T] => Result[T, P <= :T => :Bool] = @(throw: Error)` として定義されます。述語 P が真を返す場合は成功、偽を返す場合は失敗（throw 発動）を表します。

### Result

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `throw` | `Error` | エラー値（フィールド） |
| `isSuccess()` | `=> :Bool` | 成功か（述語 P が真） |
| `isError()` | `=> :Bool` | エラーか（述語 P が偽） |
| `getOrDefault(default)` | `T => :T` | 安全な値取得 |
| `getOrThrow()` | `=> :T` | 値取得（失敗時エラーを throw） |
| `map(fn)` | `:T => :U => :Result[U, _]` | モナディック変換 |
| `flatMap(fn)` | `:T => :Result[U, _] => :Result[U, _]` | モナディック連鎖 |
| `mapError(fn)` | `:Error => :Error => :Result[T, P]` | throw フィールドのエラーを変換 |
| `toString()` | `=> :Str` | 文字列表現 |
| `unmold()` | `=> :T` | アンモールディング |

---

## Async — 非同期メソッド

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `isPending()` | `=> :Bool` | 実行中か |
| `isFulfilled()` | `=> :Bool` | 完了したか |
| `isRejected()` | `=> :Bool` | 失敗したか |
| `getOrDefault(default)` | `T => :T` | 安全な値取得 |
| `map(fn)` | `:T => :U => :Async[U]` | モナディック変換 |
| `toString()` | `=> :Str` | 文字列表現 |
| `unmold()` | `=> :T` | アンモールディング |
