# リスト操作

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ

Taida のリスト `@[...]` は、同じ型の値を並べたシンプルなデータ構造です。状態の確認はメソッドで、値の操作はモールドで行います。

---

## リストリテラル `@[...]`

リストは `@[...]` で作成します。

```taida
numbers <= @[1, 2, 3, 4, 5]
names <= @["Asuka", "Rei", "Shinji"]
empty: @[Int] <= @[]
nested <= @[@[1, 2], @[3, 4]]
```

リストの要素はすべて同じ型でなければなりません。型はリテラルから自動推論されます。

```taida
// OK: 全て Int
@[1, 2, 3]

// OK: 全て Str
@["hello", "world"]

// NG: 混在はコンパイルエラーになります
// @[1, "hello"]
```

---

## 状態チェックメソッド

リストの状態を問い合わせるメソッドです。リスト自体は変更しません。

### length

要素数を返します。

```taida
@[1, 2, 3].length()           // 3
empty: @[Int] <= @[]
empty.length()                 // 0
```

### isEmpty

リストが空かどうかを返します。

```taida
empty: @[Int] <= @[]
empty.isEmpty()                // true
@[1, 2].isEmpty()              // false
```

### contains

指定した要素がリストに含まれているかを返します。

```taida
@[1, 2, 3].contains(2)   // true
@[1, 2, 3].contains(99)  // false
```

### indexOf

指定した要素の最初の出現位置を返します。見つからない場合は -1 を返します。

```taida
@[10, 20, 30, 20].indexOf(20)   // 1
@[10, 20, 30].indexOf(99)       // -1
```

### lastIndexOf

指定した要素の最後の出現位置を返します。見つからない場合は -1 を返します。

```taida
@[1, 2, 1, 2].lastIndexOf(1)   // 2
@[1, 2, 3].lastIndexOf(99)     // -1
```

---

## 安全アクセス -- Lax を返すメソッド

`first()`、`last()`、`get()`、`max()`、`min()` はすべて Lax を返します。値が存在しない場合でもプログラムは停止せず、デフォルト値にフォールバックします。

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ

### first / last

```taida fragment
@[10, 20, 30].first() >=> val   // 10
@[10, 20, 30].last() >=> val    // 30

// 空リストでも安全です
empty: @[Int] <= @[]
empty.first() >=> val            // 0 (Int のデフォルト値)
empty.last() >=> val             // 0
```

### get

インデックスを指定して要素にアクセスします。

```taida fragment
items <= @[10, 20, 30]
items.get(0) >=> val    // 10
items.get(1) >=> val    // 20
items.get(100) >=> val  // 0 (範囲外: デフォルト値)
```

### max / min

最大値・最小値を返します。

```taida fragment
@[3, 1, 4, 1, 5].max() >=> val   // 5
@[3, 1, 4, 1, 5].min() >=> val   // 1

// 空リストでも安全です
empty: @[Int] <= @[]
empty.max() >=> val                // 0
empty.min() >=> val                // 0
```

### has_value で成功/失敗を判別する

Lax の `has_value` フィールドを使うと、値が実際に存在するかどうかを判別できます。

```taida
scores <= @[85, 92, 78]

scores.get(0).has_value    // true
scores.get(100).has_value  // false

empty: @[Int] <= @[]
empty.first().has_value         // false
@[1, 2].first().has_value       // true
```

### getOrDefault でカスタムデフォルト値を指定する

```taida
@[10, 20, 30].get(100).getOrDefault(99)  // 99
empty: @[Int] <= @[]
empty.max().getOrDefault(-1)             // -1
```

---

## 述語メソッド -- any, all, none

リスト全体に対する条件判定です。Bool を返します。

### any

いずれかの要素が条件を満たすかを返します。

```taida
@[1, 2, 3, 4, 5].any(_ x = x > 3)    // true
@[1, 2, 3].any(_ x = x > 10)          // false
```

### all

すべての要素が条件を満たすかを返します。

```taida
@[2, 4, 6].all(_ x = Mod[x, 2]() >=> r; r == 0)  // true
@[1, 2, 3].all(_ x = x > 0)                        // true
@[1, 2, 3].all(_ x = x > 2)                        // false
```

### none

すべての要素が条件を満たさないかを返します。

```taida
@[1, 2, 3].none(_ x = x > 10)   // true
@[1, 2, 3].none(_ x = x > 2)    // false
```

---

## 操作モールド

> **PHILOSOPHY.md — III.** カタめたいなら、鋳型を作りましょう

リストの操作はすべてモールドで行います。結果は `>=>` で取り出すか、パイプラインで次の処理に渡します。

ここでは代表的な 4 種 (Map / Filter / Fold / Sort) の使い方を示します。リストモールドの **完全なシグネチャ一覧** (Take / Drop / Append / Prepend / Concat / Reverse / Unique / Flatten / Join / Sum / Find / FindIndex / Count / Zip / Enumerate / TakeWhile / DropWhile / Foldr) は [`docs/api/prelude.md §7.6`](../api/prelude.md#76-リストモールド) を参照してください。

### Map[list, fn]() — 各要素を変換

```taida
Map[@[1, 2, 3], _ x = x * 2]() >=> doubled
// doubled: @[2, 4, 6]

// 条件付き変換
Map[@[1, 2, 3, 4, 5], _ x =
  | x > 3 |> x * 10
  | _ |> x
]() >=> processed
// processed: @[1, 2, 3, 40, 50]
```

### Filter[list, fn]() — 条件で絞り込み

```taida
scores <= @[85, 92, 78, 95, 88]

Filter[scores, _ x: Int = x >= 90]() >=> highScores
// highScores: @[92, 95]

// 名前付き関数も使えます
isEven x: Int =
  Mod[x, 2]() >=> r
  r == 0
=> :Bool
Filter[@[1, 2, 3, 4, 5, 6], isEven]() >=> evens
// evens: @[2, 4, 6]
```

### Fold[list, init, fn]() — 畳み込み

```taida
// 合計
Fold[@[1, 2, 3, 4, 5], 0, _ acc: Int x: Int = acc + x]() >=> total
// total: 15

// 文字列の結合
names <= @["Asuka", "Rei", "Shinji"]
Fold[names, "", _ acc: Str name: Str =
  | acc == "" |> name
  | _ |> acc + ", " + name
]() >=> joined
// joined: "Asuka, Rei, Shinji"
```

### Sort[list]() — ソート

オプション `reverse` で降順、`by` でキー関数を指定できます。

```taida
Sort[@[3, 1, 4, 1, 5]]() >=> sorted
// sorted: @[1, 1, 3, 4, 5]

Sort[@[3, 1, 4, 1, 5]](reverse <= true) >=> desc
// desc: @[5, 4, 3, 1, 1]

Pilot = @(name: Str, age: Int)
pilots <= @[
  @(name <= "Asuka", age <= 14),
  @(name <= "Shinji", age <= 14),
  @(name <= "Rei", age <= 14)
]
Sort[pilots](by <= _ p: Pilot = p.age) >=> byAge
```

---

## パイプラインでのリスト操作

パイプライン `=>` / `<=` の中で `_` が前ステップの値を受け取ります。リスト操作を連鎖させることができます。

### 正方向パイプライン

```taida
scores <= @[85, 92, 78, 95, 88]

// フィルタ → マップ → 結合
scores => Filter[_, _ x: Int = x >= 90]() => Map[_, _ x: Int = x * 2]() => highDoubled
// highDoubled: @[184, 190]

// フィルタ → ソート → 先頭3件
@[5, 2, 8, 1, 9, 3, 7] => Filter[_, _ x: Int = x > 3]() => Sort[_]() => Take[_, 3]() => result
// result: @[5, 7, 8]
```

逆方向パイプラインを書きたい場合は `<=` チェーンを使えます。例:

```taida
result <= Take[_, 3]() <= Sort[_]() <= Filter[_, _ x: Int = x > 3]() <= @[5, 2, 8, 1, 9, 3, 7]
// result: @[5, 7, 8]
```

同じ文内で `=>` と `<=` を混ぜることはできません（単一方向制約）。

### 直接呼び出し（アンモールド）

パイプラインを使わず、中間変数に結果を保持する方法もあります。

```taida
numbers <= @[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]

isEven x: Int =
  Mod[x, 2]() >=> r
  r == 0
=> :Bool

Filter[numbers, isEven]() >=> evens
Map[evens, _ x: Int = x * 2]() >=> doubled
Fold[doubled, 0, _ acc: Int x: Int = acc + x]() >=> sum
// sum: 60
```

---

## パターン: データ処理パイプライン

### NERV スタッフデータの処理

```taida
Staff = @(name: Str, age: Int, role: Str, active: Bool)

staff <= @[
  @(name <= "Asuka", age <= 14, role <= "pilot", active <= true),
  @(name <= "Shinji", age <= 14, role <= "pilot", active <= true),
  @(name <= "Rei", age <= 14, role <= "pilot", active <= false),
  @(name <= "Ritsuko", age <= 30, role <= "scientist", active <= true)
]

// アクティブなスタッフの名前を取得します
staff => Filter[_, _ s: Staff = s.active]() => Map[_, _ s: Staff = s.name]() => activeNames
// activeNames: @["Asuka", "Shinji", "Ritsuko"]

// 平均年齢を計算します
staff => Map[_, _ s: Staff = s.age]() => ages
Fold[ages, 0, _ acc: Int a: Int = acc + a]() >=> totalAge
Div[totalAge, ages.length()]() >=> avgAge
// avgAge: 18
```

### 売上データの集計

```taida
Order = @(product: Str, quantity: Int, price: Int)

orders <= @[
  @(product <= "A", quantity <= 5, price <= 100),
  @(product <= "B", quantity <= 3, price <= 200),
  @(product <= "A", quantity <= 2, price <= 100)
]

// 各注文の小計を計算し、合計します
orders => Map[_, _ o: Order = o.quantity * o.price]() => subtotals
Sum[subtotals]() >=> totalRevenue
// totalRevenue: 1300

// 商品Aだけの合計
orders => Filter[_, _ o: Order = o.product == "A"]() => Map[_, _ o: Order = o.quantity * o.price]() => aSubtotals
Sum[aSubtotals]() >=> aRevenue
// aRevenue: 700
```

---

## まとめ

| 分類 | API | 返す値 |
|------|-----|--------|
| **状態チェック** | `length()`, `isEmpty()`, `contains()`, `indexOf()`, `lastIndexOf()` | Int / Bool |
| **安全アクセス** | `first()`, `last()`, `get()`, `max()`, `min()` | Lax[T] |
| **述語** | `any()`, `all()`, `none()` | Bool |
| **操作** | `Map`, `Filter`, `Fold`, `Sort`, `Reverse`, etc. | モールド |

メソッドは「聞くだけ」、モールドは「作り変える」。ゴリラリテラルは終了。この使い分けが Taida のリスト操作の基本です。

リストモールドの完全シグネチャは [`docs/api/prelude.md §7.6`](../api/prelude.md#76-リストモールド) を、メソッドの仕様は [`docs/api/prelude.md §8.5`](../api/prelude.md#85-list-メソッド) を参照してください。
