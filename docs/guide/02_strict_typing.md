# 型のガチガチさ

> **PHILOSOPHY.md -- I.** 深く考えずに適当にぶちこんでけ

---

## Taida は柔軟ではありません

他の言語は柔軟さを売りにすることが多いですが、Taida は**ガチガチな厳格さ**を売りにしています。

柔軟さは「何が起こるかわからない」の別名であり、暗黙変換は「予想外の挙動」の温床です。Taida の型システムは、開発者が考えなくていいように全てを決めています。デフォルト値があり、暗黙変換はなく、失敗は Lax で安全に処理されます。曖昧さはありません。

Taida の「ガチガチ」は、エラーで止めるのではなく、**Lax で安全に値を返す**ことでガチガチさと安全性を両立しています。

---

## 暗黙の型変換は存在しません

```taida
// 他の言語ならこう書けます。Taida ではコンパイルエラーになります。
"Count: " + 42        // コンパイルエラー: Str と Int の加算はできません

// これが正解です。明示的に変換してください。
"Count: " + 42.toString()  // "Count: 42"
"Count: " + Str[42]() ]=> _  // Str モールドでも OK です
```

JavaScript は `"Count: " + 42` を `"Count: 42"` にしてくれます。便利ですが、`"5" - 3` は `2` になり、`"5" + 3` は `"53"` になります。便利の裏に予想外の挙動が潜んでいます。

Python は `"Count: " + 42` で `TypeError` を出します。正しい判断ですが、それは実行時エラーであり、プログラムを走らせるまでわかりません。

Taida は**コンパイル時**にエラーにします。走る前に止めます。

```taida
// 全て明示的に行います
num <= 42
str <= num.toString()           // "42"
Int["42"]() ]=> back            // 42
ToFixed[num, 2]() ]=> formatted // "42.00"
```

---

## ゼロ除算は Lax で安全に処理します

Taida に `/` 演算子と `%` 演算子はありません。除算と剰余は `Div[x, y]()` と `Mod[x, y]()` モールドで行います。ゼロ除算は Lax を返し、プログラムを停止させません。

```taida fragment
Div[10, 3]() ]=> result  // 3
Div[10, 0]() ]=> result  // 0 (ゼロ除算: Lax のデフォルト値)
```

### 他の言語との比較

| 言語 | `10 / 0` の結果 | 問題 |
|------|-----------------|------|
| JavaScript | `Infinity` | 壊れたまま走り続けます |
| Python | `ZeroDivisionError` | 実行時エラー |
| Java | `ArithmeticException` (int), `Infinity` (double) | 型によって挙動が異なります |
| Rust | パニック (int), `Infinity` (float) | 型によって挙動が異なります |
| **Taida** | **Lax (デフォルト値)** | **常に値を返します。hasValue で判別できます** |

Taida はゼロ除算で壊れません。`Div[x, y]()` は Lax を返し、成功時は計算結果が、失敗時は型のデフォルト値がアンモールディングで取り出されます。

成功/失敗を判別する必要がある場合は `hasValue` を参照してください:

```taida
lax <= Div[10, 0]()
lax.hasValue  // false (ゼロ除算は失敗)

lax2 <= Div[10, 3]()
lax2.hasValue  // true (正常に割れた)
```

剰余も同様です:

```taida fragment
Mod[10, 3]() ]=> result   // 1
Mod[10, 0]() ]=> result   // 0 (ゼロ除算: デフォルト値)
Mod[10, 0]().hasValue      // false
```

---

## 範囲外アクセスは Lax で安全に処理します

リストの要素アクセスは `.get()` メソッドで行います。範囲外の場合は Lax を返し、プログラムを停止させません。

```taida fragment
items <= @[10, 20, 30]
items.get(1) ]=> val    // 20
items.get(100) ]=> val  // 0 (範囲外: Lax のデフォルト値)
```

### 他の言語との比較

| 言語 | `array[100]` の結果 | 問題 |
|------|---------------------|------|
| JavaScript | `undefined` | 黙って壊れます |
| Python | `IndexError` | 実行時エラー |
| Java | `ArrayIndexOutOfBoundsException` | 実行時エラー |
| Rust | パニック | |
| **Taida** | **Lax (デフォルト値)** | **常に値を返します。hasValue で判別できます** |

`first()`、`last()`、`max()`、`min()` も同様に Lax を返します:

```taida fragment
@[1, 2, 3].first() ]=> val  // 1
empty: @[Int] <= @[]
empty.first() ]=> val        // 0 (空リスト: Lax のデフォルト値)

@[1, 2, 3].last() ]=> val   // 3
empty.last() ]=> val         // 0 (空リスト: Lax のデフォルト値)

@[1, 3, 2].max() ]=> val    // 3
empty.max() ]=> val          // 0 (空リスト: Lax のデフォルト値)
```

成功/失敗を判別したい場合:

```taida
empty: @[Int] <= @[]
lax <= empty.first()
lax.hasValue  // false (空リスト)
```

---

## JSON はエアロック（溶鉄）

外部から入ってくる JSON データは、Taida の型安全な世界にそのまま入れることはできません。JSON は**溶鉄**です。形がなく、メソッドを持たず、スキーマ（鋳型）を通さなければ使えません。

```
外部世界 (JSON)           鋳型（スキーマ）         Taida の型安全な世界
                      +-----------+
  {"name": "Asuka"}  |           |    name: Str <= "Asuka"
  {"age": null}   --> | JSON[x,T] | --> age: Int <= 0 (デフォルト値)
  {"x": ???}          |           |    (余分なフィールドは無視)
                      +-----------+
```

### JSON 構文

```taida
// スキーマを定義します
Pilot = @(name: Str, age: Int, sync_rate: Int)

// JSON[生データ, スキーマ]() で鋳造します
raw <= '{"name": "Shinji", "age": 14, "sync_rate": 78}'
JSON[raw, Pilot]() ]=> pilot

pilot.name      // "Shinji"
pilot.age       // 14
pilot.sync_rate  // 78
```

### スキーマなしはエラーです

```taida fragment
// NG: スキーマなし
JSON[raw]()               // コンパイルエラー: JSON requires a schema type argument

// NG: JSON にメソッドはありません
data <= JSON[raw, Pilot]()
data.at("name")           // コンパイルエラー: JSON has no methods

// OK: スキーマを通して鋳造します
JSON[raw, Pilot]() ]=> pilot
pilot.name                // "Shinji" (型安全)
```

### null は排除されます

```taida
Pilot = @(name: Str, age: Int)

// JSON の null はデフォルト値に変換されます
raw <= '{"name": null, "age": null}'
JSON[raw, Pilot]() ]=> pilot
// pilot = @(name <= "", age <= 0)
```

### 出力方向は安全です

Taida の値を JSON 文字列にする方向は型安全なので、自由に使えます:

```taida
jsonEncode(pilot)   // JSON 文字列に変換
jsonPretty(pilot)   // 整形された JSON 文字列に変換
```

---

## デフォルト値保証

> **PHILOSOPHY.md -- I.** 深く考えずに適当にぶちこんでけ

「安全に処理する」と「デフォルト値がある」は一貫しています。

- `Div[10, 0]()` は Lax を返します -- これは**よくある失敗**なので Lax でデフォルト値にフォールバックします
- `Pilot(name <= "Rei")` で `age` は `0` -- これは**省略されたフィールド**なのでデフォルト値を使います
- `JSON[raw, Pilot]()` でフィールド欠損 → デフォルト値 -- 外部データの不備はデフォルトで吸収します

よくある失敗には Lax で安全に処理し、省略された値にはデフォルトを与えます。全ての操作で「値が存在する」ことを保証するのが Taida の型システムの核心です。

| 型 | デフォルト値 |
|----|-------------|
| Int | `0` |
| Float | `0.0` |
| Str | `""` |
| Bool | `false` |
| @[T] | `@[]` |
| JSON | `{}` |

null はありません。undefined もありません。全ての変数には値があります。

---

## 型のガチガチさ = 安全 + 明示的

Taida の型のガチガチさは2つの意味を持っています。

### 1. 全てが明示的であること

```taida fragment
// 暗黙変換はありません
"Count: " + 42                 // コンパイルエラー
"Count: " + 42.toString()      // OK: 明示的な変換

// 操作はモールドで明示的に行います
Upper["hello"]()               // "HELLO"（モールドで操作）
Div[10, 3]()                   // 除算もモールドで

// JSON はスキーマ必須です
JSON[raw, Pilot]()             // OK: スキーマ指定
JSON[raw]()                    // NG: スキーマなし
```

### 2. 全ての操作が安全であること

```taida
// ゼロ除算 → Lax で安全に
Div[10, 0]() ]=> result        // 0 (プログラムは止まりません)

// 範囲外アクセス → Lax で安全に
@[1, 2].get(100) ]=> val      // 0 (プログラムは止まりません)

// 型変換失敗 → Lax で安全に
Int["abc"]() ]=> num           // 0 (プログラムは止まりません)

// JSON のフィールド欠損 → デフォルト値
JSON['{}', Pilot]() ]=> pilot  // 全フィールドがデフォルト値
```

### 型チェッカーが守るもの

```taida
// 存在しないフィールドへのアクセス
pilot <= @(name <= "Asuka")
call_sign <= pilot.call_sign  // コンパイルエラー: フィールド 'call_sign' は存在しません

// 型不一致
add x: Int y: Int = x + y => :Int
result <= add("hello", 5)  // コンパイルエラー: 第1引数で Str 型を Int 型に適用できません

// 単一方向制約
data => Filter[_, _ x = x > 0]() <= result  // コンパイルエラー: 単一方向制約違反
```

---

## ガチガチの何がいいのか

1. **バグが実行前にわかります** -- コンパイル時に型エラーを検出します
2. **よくある失敗で止まりません** -- Lax がデフォルト値で安全に処理します
3. **AI が安全なコードを生成できます** -- 暗黙の挙動がないため、AI の出力が予測可能です
4. **人間は眺めるだけで構いません** -- 型が合っていれば、コードの構造が正しいことが保証されます

> **PHILOSOPHY.md -- IV.** キモい言語だ。だが、書くのも読むのもAI。眺めるのが人間の仕事。

ガチガチだからこそ、安心してぶちこめるのです。

---

## まとめ

| 原則 | Taida の対応 |
|------|-------------|
| 暗黙変換 | 存在しません。コンパイルエラーです |
| ゼロ除算 | `Div[x, y]()` が Lax を返します。hasValue で判別できます |
| 範囲外アクセス | `.get()` が Lax を返します。hasValue で判別できます |
| 型変換 | モールドで明示的に。`Int[x]()`, `Str[x]()` 等。全て Lax を返します |
| JSON | 溶鉄。`JSON[raw, Schema]()` でスキーマ必須。メソッドなし |
| null | 存在しません。全ての型にデフォルト値があります |
| カスタムエラー | throw + ゴリラ天井。Lax とは別の仕組みです |
