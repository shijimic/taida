# モールド

> **PHILOSOPHY.md — III.** カタめたいなら、鋳型を作りましょう

モールドは「値を流し込んで取り出す鋳型」です。型パラメータ化された変換・ラッパー・失敗チャネル付き値を、共通の構文で扱うための仕組みです。

本章ではモールド型の **解剖**・**ユーザー定義**・**主要なモールド型** (Lax / Result / Gorillax / Cage) の概念を扱います。文字列・数値・リスト・型変換などの **個別モールドのリファレンス** は [`docs/api/prelude.md`](../api/prelude.md) に集約しています。

---

## モールドとは

型パラメータ化が必要になったら、鋳型 (Mold) を作ります。値を鋳型に流し込み (モールド)、必要なときに取り出します (アンモールド)。

操作はモールドで、状態チェックはメソッドで -- これが Taida の原則です。

```taida fragment
// クラスライク型として鋳型を定義する (詳細: 04_class_like.md)
Mold[T] => Result[T, P <= :T => :Bool] = @(throw: Error)

// 値を流し込みます
boxed <= Result[42, _ = true]()

// 取り出します
boxed >=> value  // 42
```

> 鋳型自体の定義 (`Mold[T] => Foo[T] = @(...)`) は [クラスライク型定義](04_class_like.md) の単一構文の一例です。本章では既に定義された鋳型を **使う** 側 (流し込み・取り出し操作) に集中します。

---

## モールドの解剖

クラスライク型としての鋳型の構造を簡単に振り返ります (詳細は [04_class_like.md](04_class_like.md) を参照)。

```taida
// 基本形式 (クラスライク型定義)
Mold[T] => MyMold[T] = @(
  filling: T, // [T]に代入された値が格納される
  unmold _ = filling => :T // 取り出し値 (>=> / <=< / .unmold()) を決める

  // 追加フィールドを定義 (プロパティ、メソッド)
)
```

ヘッダー (`Mold[T] => MyMold[T]`) は親型と型引数の数の一致が要件です ([04_class_like.md](04_class_like.md#親型の型引数と引数の数の一致))。`Mold[...]` の親側は常に 1 スロットのままで、追加スロットは子側 `MyMold[T, U]` 等で書きます。

ヘッダースロットの意味:

- 1 つ目のスロットは常に `filling` に対応します
- 2 つ目以降のスロットは、`@(...)` 内の「デフォルト値なしフィールド」に宣言順で対応します
- この対応は `T` のような型変数だけでなく `:Int` のような具体型スロットでも同じです
- つまり、具体型スロットを途中に置いた場合も、そのスロットは 1 つの束縛先フィールドを消費します

```taida fragment
Mold[:Int] => IntBox = @(value: Int)
ok_box <= IntBox[1]()
// IntBox["x"]() はコンパイルエラー: 1 つ目のスロットは具体型 Int

Mold[:Int] => IntPair[:Int, T] = @(
  second: T
)
pair <= IntPair[1, "x"]()

// NG: 空 body は `[E1520]` で reject される
// Mold[:Int] => Broken[:Int, T] = @()
```

`[E1520]` は「値の不在を表す型の完全排除」診断で、`@()` を ClassLike / Mold / Inheritance の **body** として書くこと自体を Parser が即時 reject します。子型を作るときは意味のあるフィールドを必ず 1 つ以上置きます (例: `Mold[:Int] => IntBox = @(value: Int)`)。空リスト `@[]` は「空のリスト」という別意味を持つため、`[E1520]` の対象にはなりません。

### 具体型スロットには `:` を必ず付ける

ヘッダースロットの記法は、**具体型** か **型変数** かで明確に書き分けます。

| 形式 | 意味 | 例 |
|------|------|------|
| `Mold[:Int]` | 具体型 `Int` 専用の鋳型 | `Mold[:Int] => IntBox = @(value: Int)` |
| `Mold[T]` | 任意の型を受ける鋳型 (型変数 `T`) | `Mold[T] => Box[T] = @(value: T)` |
| `Mold[T <= :Int]` | 具体型制約付き型変数 | `Mold[T <= :Int] => NumBox[T] = @(value: T)` |

**`:` を欠いた具体型** (`Mold[Int]`) は、組み込み型名 (`Int` / `Str` / `Bool` / `Float` / `Bytes` / `Lax` / `Result` / `Async` / `Optional` 等) と衝突する型変数名として書かれた場合、型チェッカーが `[E1523]` で reject します。`Mold[Int] => MyMold[Int] = @(value: Int)` は文法上は通りますが、`Int` 型変数の意図か具体型 `Int` の意図か曖昧なため、明示的に `Mold[:Int]` (具体型直接指定) または `Mold[T <= :Int]` (制約付き型変数) のどちらかに書き換える必要があります。**具体型を使うときは必ず `:` を付ける** と覚えてください。なお Mold body は `[E1520]` の対象なので、body の `@()` 部分は意味のあるフィールドを必ず 1 つ以上含めます。

### unmold フック

`Mold` の取り出し挙動は `unmold` フックで決まります。モールド値そのものは常に **モールドパック自身** です (フィールドと型識別を保持したまま固まる)。

| フック | デフォルト | 意味 |
|---|---|---|
| `unmold` | `filling` を返す | 固まった値から何を取り出すか |

`Name[args]()` の評価は次の順序です。

1. `[]` を `filling` とデフォルト値なしフィールドへ順に束縛
2. `()` をデフォルト値ありフィールドへ束縛
3. `Name[args]()` の式値はモールドパック自身

演算子の意味:

- `Name[args]() => x`: モールドパックを `x` に代入
- `Name[args]() >=> x`: モールドパックに対して `unmold` を実行して `x` に代入

フックの定義形は `unmold _ = <式> => :<取り出し型>` です。式からは同じモールドのフィールドを直接参照できます。

### `filling` と引数バインド規則

`filling` は常に 1 つ目の `[]` 位置引数です。2 つ目以降は `@(...)` のフィールド定義から自動的に割り当てられます。

| フィールド種別 | 入力 | 省略 |
|---|---|---|
| `filling` | 1 つ目の `[]` | 不可 |
| デフォルト値なしフィールド (`filling` 以外) | 2 つ目以降の `[]` (宣言順) | 不可 |
| デフォルト値ありフィールド | `()` 名前付き設定 | 可 |

規則違反はコンパイルエラーです (不足・過多・`[]` / `()` の取り違え・未定義オプション)。
また、カスタムモールド定義時に追加型引数の束縛先が無い場合もコンパイルエラーです。
さらに、通常フィールドは `field: Type` または `field <= value` のどちらかが必要です (`field` 単独は不可)。

```taida
Mold[T] => Ratio[T, U] = @(
  divisor: U,
  unmold _ = filling => :T
)

stdout(Ratio[10, 3]())  // filling=10, divisor=3
```

```taida reject
Mold[T] => Ratio[T, U] = @(
  divisor: U,
  unmold _ = filling => :T
)
Ratio[10]()     // コンパイルエラー: divisor が不足

Mold[T] => Broken[T, U] = @(
  unmold _ = filling => :T
)
// コンパイルエラー: U の束縛先が無い

// 空 body は `[E1520]` で reject されるため、ヘッダーアリティが合っていても
// body 側にフィールドを書く必要がある:
// Mold[:Int] => AlsoBroken[:Int, U] = @()  // [E1520] empty class-like body

Mold[T] => BrokenField[T] = @(
  count
)
// コンパイルエラー: count は型注釈かデフォルト値が必要
```

### インスタンス化

実際の値を型引数として渡すと、型が自動推論されます。

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
| **役割** | 必須引数 (何を / 何で) | オプション設定 (どうやって) |
| **名前** | なし (位置で区別) | あり (名前で区別) |
| **省略** | 不可 | 可 (デフォルト値あり) |
| **順序** | 固定 (`[何を, 何で]`) | 任意 |

```taida
// [] = 必須引数: str と old と new がなければ置換不可能
// () = オプション: all のデフォルトは false (最初の 1 つだけ)
Replace["hello world", "world", "taida"](all <= true)

// [] = 必須引数: list がなければソート不可能
// () = オプション: reverse と by にはデフォルトがある
Sort[@[3, 1, 2]](reverse <= true)
```

---

## 継承時のオーバーライド

クラスライク継承では、子型は親型のフィールドを以下のルールで上書きできます。

| ケース | 振る舞い |
|---|---|
| 同名フィールド・同一型で再宣言 | 冪等 (許可、最後の宣言が有効) |
| 同名フィールド・異なる型で再宣言 | コンパイルエラー `[E1411]` |
| 親に無いフィールドを子で追加 | 許可 (子型で新規追加) |
| `unmold` のオーバーライド | 子型で `unmold _ = <式> => :U` を再宣言すれば上書き可 |

```taida
Mold[T] => Box[T] = @(
  filling: T,
  unmold _ = filling => :T
)

// unmold のオーバーライド: 子型で別の取り出し値を選ぶ
Box[T] => Doubled[T] = @(
  unmold _ = filling * 2 => :Int
)
Doubled[21]() >=> v
stdout(v)  // 42
```

```taida reject
Mold[T] => Box[T] = @(
  filling: T,
  unmold _ = filling => :T
)

// 同名フィールドの型不一致は [E1411]
Box[T] => Bad[T] = @(
  filling: Str   // 親 Box[T] の filling: T と衝突
)
// コンパイルエラー [E1411]: redefines field with incompatible type
```

declare-only な関数フィールド (`fn: T => :U` 本体なし) は親で宣言し、子で実装するパターンに使えます。詳細は [クラスライク型定義 / declare-only 関数フィールド](04_class_like.md#declare-only-関数フィールド) を参照してください。

---

## アンモールド

モールド型から値を取り出すには 3 つの方法があります。
`Name[args]()` に対する `>=>` / `<=<` / `.unmold()` は、固まったモールドパックに対して実行されます。

### `>=>` 演算子

```taida
lax <= Div[10, 3]()
lax >=> value  // value = 3
```

### `<=<` 演算子 (逆向き)

```taida fragment
value <=< lax  // value = 3
```

### `.unmold()` メソッド

```taida fragment
value <= lax.unmold()  // value = 3
```

### 使い分け

```taida fragment
// 代入したい場合は演算子を使います
lax >=> value

// 式の中で使いたい場合はメソッドを使います
result <= lax.unmold() + 10
```

---

## パイプラインでのモールド使用

パイプライン `=>` の中では、`_` が前ステップの値を受け取ります。モールドの `[]` 内でも使用可能で、同一ステップ内なら **複数回参照** しても全て同じ前段値に bind されます。

```taida
"  Hello, World!  " => Trim[_]() => Upper[_]() => result
// result: "HELLO, WORLD!"

// 同じ _ を複数回参照する例 (clamp パターン)
150 => If[_ > 100, 100, _]() => clamped   // 100
```

`_` プレースホルダの全体仕様 (パイプライン内 / 関数引数の `f(a, , c)` カンマスキップ等) は [演算子リファレンスの `_` プレースホルダ節](../reference/operators.md#_-プレースホルダ) を参照してください。

`<=` チェーンによる逆方向パイプライン（例: `result <= If[_ > 100, 100, _]() <= 150`）も正式構文として利用できます。同じ文内で `=>` と混ぜることはできません。

---

## ユーザー定義モールド

`Mold[T]` を継承して独自のモールド型を定義できます。これは [クラスライク型定義](04_class_like.md) の単一構文の一例です。

```taida
Mold[:@(x: Int, y: Int)] => Container = @(
  count: Int,
  name: Str,
  unmold _ = filling => :@(x: Int, y: Int) // unmold のカスタム定義
)

data <= @(x <= 1, y <= 2)
box <= Container[data, 1, "my-container"]()
box >=> extracted  // @(x <= 1, y <= 2)
box.count          // 1
box.name           // "my-container"
```

### unmold で別の型を取り出す

`unmold` をオーバーライドすると、カスタムモールドから `filling` 以外の値を取り出せます。

```taida
Mold[T] => TryInt[T] = @(
  unmold _ = Int[filling]().getOrDefault(0) => :Int
)

TryInt["123"]() => boxed   // boxed: モールドパック (filling を保持)
TryInt["123"]() >=> value  // value: 123 (unmold の評価結果)
```

### メソッドの定義

モールド型内にメソッドを定義できます。

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

declare-only 関数フィールド (`fn: T => :U` 本体なし) は全クラスライク系統 (Mold / Error 含む) で利用できます。defaultFn による自動充足の挙動については [クラスライク型定義](04_class_like.md#declare-only-関数フィールド) を参照してください。

---

## 主要なモールド型 — 概念紹介

Taida の標準ライブラリは、用途別に複数のモールド型を提供しています。ここでは概念と使い分けを紹介し、個別のシグネチャは [`docs/api/prelude.md`](../api/prelude.md) に集約しています。

### Lax[T] — 必ず値を返すモールド型

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ

Lax は「操作が失敗しても必ず値を返す」モールド型です。失敗時は型 `T` のデフォルト値にフォールバックします。

```taida
// 成功: has_value = true, アンモールドで値が取り出せる
Div[10, 3]() >=> quotient   // quotient = 3

// 失敗: has_value = false, アンモールドでデフォルト値が返る
Div[10, 0]() >=> fallback   // fallback = 0 (Int のデフォルト値)
Div[10, 0]().has_value  // false
```

Lax を返す代表的な操作: `Div` / `Mod`、型変換 (`Int` / `Float` / `Str` / `Bool`)、`JSON[raw, Schema]()`、リストアクセス (`.get(idx)` / `.first()` / `.last()` / `.max()` / `.min()`)、`Find[list, fn]()`。詳細は [`docs/api/prelude.md`](../api/prelude.md) を参照してください。

### Result[T, P] — 述語付き操作モールド

成功 / 失敗を **述語 (`P: :T => :Bool`)** で判定するモールド型です。`>=>` でアンモールドすると述語が評価され、成功なら値 `T` を返し、失敗なら `throw` が発動します。

```taida fragment
Error => ValidationError = @(field: Str)

age <= 15
checked <= Result[age, _ x = x >= 18](throw <= ValidationError(type <= "ValidationError", message <= "Must be 18+", field <= "age"))
checked >=> value  // age=15 → 述語が false → throw 発動
```

`map` / `flatMap` / `mapError` などモナディック操作はメソッドとして提供されます。

### Gorillax[T] — 覚悟のモールド型

Lax が「失敗してもデフォルト値で続行」なのに対し、Gorillax は「失敗したらゴリラがプログラムを止める」モールド型です。安全を保証しない外部操作 (npm パッケージの利用など) に使います。

```taida
Gorillax[42]() => g   // g: Gorillax[Int], has_value = true
g >=> value           // 42 (成功: 値が取り出せる)
```

`.relax()` を呼ぶと `RelaxedGorillax[T]` に変換され、失敗が `RelaxedGorillaEscaped` エラーとして `|==` エラー天井で捕捉できるようになります。

### Cage[subject, runner] — 外部世界との境界

Cage は外部由来の Molten 値を扱うための **境界 (boundary)** です。`Cage[subject, runner]()` は subject の Molten branch と runner の `CageRilla[Branch, Out]` 子系統 descriptor の `Branch` を型レベルで照合します。一致したときだけ branch operation を実行し、同期 runner は `Gorillax[Out]`、Promise を返す JS runner は `Async[Out]` で返します。

```taida fragment
>>> npm:lodash => @(lodash)

items: @[Int] <= @[1, 2, 3, 4, 5]
Cage[lodash, JSCall[@["sum"], @[items], Int]()]() >=> total
// total: Int = 15
```

```taida fragment
>>> npm:node:timers/promises => @(setTimeout)

Cage[setTimeout, JSCallAsync[@[], @[20, 42], Int]()]() >=> value
// value: Int = 42
```

CageRilla 子系統と JSRilla 系 constructor (`JSGet` / `JSCall` / `JSCallAsync` / `JSNew` / `JSSet` / `JSBind` / `JSSpread`) の詳細は [`docs/api/js.md`](../api/js.md) を参照してください。`JSRilla` 子系統は **旧 JS ターゲット専用** で、インタプリタ・Native・WASM では利用できません。

JSON のような structured data の schema cast は **`Cage` 経路を通りません**。JSON 専用 facade `JSON[raw, Schema]()` が `Lax[T]` failure channel を維持します。

| 状況 | 使うべきもの | failure channel |
|------|------------|-----------------|
| ゼロ除算 / 範囲外アクセス / 型変換 | Lax (デフォルト値で続行) | `Lax(false)` |
| JSON 文字列 → 型付き値 | `JSON[raw, Schema]()` | `Lax(false)` |
| npm パッケージの同期 Molten 操作 | `Cage[subject, JSCall[...]()]()` | `Gorillax[Out]` |
| Promise を返す JS 操作 | `Cage[subject, JSCallAsync[...]()]()` | `Async[Out]` |
| 外部操作の失敗をキャッチ | `.relax()` → `RelaxedGorillax` | `|==` エラー天井 |

---

## まとめ

| 概念 | 構文 |
|------|------|
| 鋳型の定義 | `Mold[T] => TypeName[T] = @(...)` ([04_class_like.md](04_class_like.md)) |
| 中身 | `filling: T` |
| 固まる値 | 常にモールドパック自身 (フィールドと型識別を保持) |
| 取り出し定義 | `unmold _ = <式> => :U` (省略時: filling) |
| インスタンス化 | `Name[a, b, ...](opt <= ...)` (`a` は filling、`b...` はデフォルト値なしフィールド宣言順) |
| アンモールド | `>=>`, `<=<`, `.unmold()` |
| `[]` 位置引数 | 必須引数 (何を / 何で) |
| `()` 名前付き設定 | オプション設定 (どうやって) |
| 継承オーバーライド | 同名同型 冪等 / 同名異型 `[E1411]` / `unmold` 上書き可 |
| パイプライン | `=>` の中で `_` が前ステップの値を受け取る (同一ステップで複数参照可) |

個別モールド (文字列 / 数値 / リスト / 型変換 / 演算 / 条件 / 型比較) は [`docs/api/prelude.md`](../api/prelude.md) を参照してください。
