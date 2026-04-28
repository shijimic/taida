# リスト操作

> **PHILOSOPHY.md -- I.** 深く考えずに適当にぶちこんでけ

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

> **PHILOSOPHY.md -- I.** 深く考えずに適当にぶちこんでけ

### first / last

```taida
@[10, 20, 30].first() ]=> val   // 10
@[10, 20, 30].last() ]=> val    // 30

// 空リストでも安全です
empty: @[Int] <= @[]
empty.first() ]=> val            // 0 (Int のデフォルト値)
empty.last() ]=> val             // 0
```

### get

インデックスを指定して要素にアクセスします。

```taida
items <= @[10, 20, 30]
items.get(0) ]=> val    // 10
items.get(1) ]=> val    // 20
items.get(100) ]=> val  // 0 (範囲外: デフォルト値)
```

### max / min

最大値・最小値を返します。

```taida
@[3, 1, 4, 1, 5].max() ]=> val   // 5
@[3, 1, 4, 1, 5].min() ]=> val   // 1

// 空リストでも安全です
empty: @[Int] <= @[]
empty.max() ]=> val                // 0
empty.min() ]=> val                // 0
```

### hasValue で成功/失敗を判別する

Lax の `hasValue` フィールドを使うと、値が実際に存在するかどうかを判別できます。

```taida
scores <= @[85, 92, 78]

scores.get(0).hasValue    // true
scores.get(100).hasValue  // false

empty: @[Int] <= @[]
empty.first().hasValue         // false
@[1, 2].first().hasValue       // true
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
@[2, 4, 6].all(_ x = Mod[x, 2]() ]=> r; r == 0)  // true
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

> **PHILOSOPHY.md -- III.** カタめたいなら、鋳型を作りましょう

リストの操作はすべてモールドで行います。結果は `]=>` で取り出すか、パイプラインで次の処理に渡します。

### 変換: Map, Filter

#### Map[list, fn]()

各要素に関数を適用して新しいリストを返します。

```taida
Map[@[1, 2, 3], _ x = x * 2]() ]=> doubled
// doubled: @[2, 4, 6]

// 条件付き変換
Map[@[1, 2, 3, 4, 5], _ x =
  | x > 3 |> x * 10
  | _ |> x
]() ]=> processed
// processed: @[1, 2, 3, 40, 50]
```

#### Filter[list, fn]()

条件を満たす要素だけを抽出します。

```taida
scores <= @[85, 92, 78, 95, 88]

Filter[scores, _ x = x >= 90]() ]=> highScores
// highScores: @[92, 95]

// 名前付き関数も使えます
isEven x =
  Mod[x, 2]() ]=> r
  r == 0
=> :Bool
Filter[@[1, 2, 3, 4, 5, 6], isEven]() ]=> evens
// evens: @[2, 4, 6]
```

### 畳み込み: Fold, Foldr

#### Fold[list, init, fn]()

左から畳み込みます。`init` が初期値、`fn` がアキュムレータ関数です。

```taida
// 合計
Fold[@[1, 2, 3, 4, 5], 0, _ acc x = acc + x]() ]=> total
// total: 15

// 文字列の結合
names <= @["Asuka", "Rei", "Shinji"]
Fold[names, "", _ acc name =
  | acc == "" |> name
  | _ |> acc + ", " + name
]() ]=> joined
// joined: "Asuka, Rei, Shinji"
```

#### Foldr[list, init, fn]()

右から畳み込みます。

```taida
Foldr[@["a", "b", "c"], "", _ acc x = x + acc]() ]=> concat
// concat: "abc"
```

### 取得: Take, Drop, TakeWhile, DropWhile

#### Take[list, n]() / Drop[list, n]()

先頭から n 個取得、または先頭 n 個をスキップします。

```taida
numbers <= @[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]

Take[numbers, 3]() ]=> first3    // @[1, 2, 3]
Drop[numbers, 3]() ]=> rest      // @[4, 5, 6, 7, 8, 9, 10]
```

#### TakeWhile[list, fn]() / DropWhile[list, fn]()

条件を満たす間だけ取得、またはスキップします。

```taida
numbers <= @[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]

TakeWhile[numbers, _ x = x < 5]() ]=> underFive  // @[1, 2, 3, 4]
DropWhile[numbers, _ x = x < 5]() ]=> fromFive   // @[5, 6, 7, 8, 9, 10]
```

### 追加: Append, Prepend, Concat

```taida
Append[@[1, 2], 3]() ]=> appended         // @[1, 2, 3]
Prepend[@[2, 3], 1]() ]=> prepended       // @[1, 2, 3]
Concat[@[1, 2], @[3, 4]]() ]=> combined   // @[1, 2, 3, 4]
```

### 整形: Sort, Reverse, Unique, Flatten

#### Sort[list]()

要素をソートします。オプションで降順やキー関数を指定できます。

```taida
Sort[@[3, 1, 4, 1, 5]]() ]=> sorted
// sorted: @[1, 1, 3, 4, 5]

// 降順
Sort[@[3, 1, 4, 1, 5]](reverse <= true) ]=> desc
// desc: @[5, 4, 3, 1, 1]

// キー関数でソート
pilots <= @[
  @(name <= "Asuka", age <= 14),
  @(name <= "Shinji", age <= 14),
  @(name <= "Rei", age <= 14)
]
Sort[pilots](by <= _ p = p.age) ]=> byAge
```

| オプション | デフォルト | 説明 |
|-----------|----------|------|
| `reverse` | `false` | `true` で降順 |
| `by` | なし（自然順） | キー抽出関数 |

#### Reverse[list]()

```taida
Reverse[@[1, 2, 3]]() ]=> reversed  // @[3, 2, 1]
```

#### Unique[list]()

重複を除去します。

```taida
Unique[@[1, 2, 2, 3, 3, 1]]() ]=> uniq  // @[1, 2, 3]

// キー関数で重複判定
Unique[items](by <= _ x = x.id) ]=> uniqueById
```

#### Flatten[list]()

ネストしたリストを1段階フラット化します。

```taida
Flatten[@[@[1, 2], @[3, 4]]]() ]=> flat  // @[1, 2, 3, 4]
```

### 結合: Join, Sum

#### Join[list, sep]()

要素を区切り文字で結合して文字列にします。

```taida
Join[@["a", "b", "c"], ","]() ]=> csv  // "a,b,c"
Join[@[1, 2, 3], "-"]() ]=> dashed     // "1-2-3"
```

#### Sum[list]()

数値リストの合計を返します。

```taida
Sum[@[1, 2, 3, 4, 5]]() ]=> total  // 15
empty: @[Int] <= @[]
Sum[empty]() ]=> zero               // 0
```

### 検索: Find, FindIndex, Count

#### Find[list, fn]()

条件を満たす最初の要素を Lax で返します。

```taida
Find[@[1, 2, 3, 4, 5], _ x = x > 3]() ]=> found
// found: 4

Find[@[1, 2, 3], _ x = x > 10]().hasValue  // false
```

#### FindIndex[list, fn]()

条件を満たす最初の要素の位置を返します。見つからない場合は -1。

```taida
FindIndex[@[10, 20, 30, 40], _ x = x > 25]()  // 2
```

#### Count[list, fn]()

条件を満たす要素の数を返します。

```taida
Count[@[1, 2, 3, 4, 5], _ x = x > 2]()  // 3
```

### ペア: Zip, Enumerate

#### Zip[list, other]()

2つのリストを要素ごとにペアにします。短い方に合わせます。

```taida
Zip[@[1, 2, 3], @["a", "b", "c"]]() ]=> pairs
// pairs: @[@(first <= 1, second <= "a"), @(first <= 2, second <= "b"), @(first <= 3, second <= "c")]
```

#### Enumerate[list]()

各要素にインデックスを付与します。

```taida
Enumerate[@["Asuka", "Rei", "Shinji"]]() ]=> indexed
// indexed: @[@(index <= 0, value <= "Asuka"), @(index <= 1, value <= "Rei"), @(index <= 2, value <= "Shinji")]
```

---

## パイプラインでのリスト操作

パイプライン `=>` / `<=` の中で `_` が前ステップの値を受け取ります。リスト操作を連鎖させることができます。

### 正方向パイプライン

```taida
scores <= @[85, 92, 78, 95, 88]

// フィルタ → マップ → 結合
scores => Filter[_, _ x = x >= 90]() => Map[_, _ x = x * 2]() => highDoubled
// highDoubled: @[184, 190]

// フィルタ → ソート → 先頭3件
@[5, 2, 8, 1, 9, 3, 7] => Filter[_, _ x = x > 3]() => Sort[_]() => Take[_, 3]() => result
// result: @[5, 7, 8]
```

### 中間変数による逆順処理

```taida
// 順方向パイプラインで同じ結果を得られます
@[5, 2, 8, 1, 9, 3, 7] => Filter[_, _ x = x > 3]() => Sort[_]() => Take[_, 3]() => result
```

> **注意**: `<=` のチェーンによる逆方向パイプライン（`result <= f() <= g() <= data`）は仕様上有効ですが、現在のパーサーでは単一の `<=` 代入のみサポートしています。順方向パイプライン `=>` または中間変数を使用してください。

### 直接呼び出し（unmold）

パイプラインを使わず、中間変数に結果を保持する方法もあります。

```taida
numbers <= @[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]

isEven x =
  Mod[x, 2]() ]=> r
  r == 0
=> :Bool

Filter[numbers, isEven]() ]=> evens
Map[evens, _ x = x * 2]() ]=> doubled
Fold[doubled, 0, _ acc x = acc + x]() ]=> sum
// sum: 60
```

---

## パターン: データ処理パイプライン

### NERV スタッフデータの処理

```taida
staff <= @[
  @(name <= "Asuka", age <= 14, role <= "pilot", active <= true),
  @(name <= "Shinji", age <= 14, role <= "pilot", active <= true),
  @(name <= "Rei", age <= 14, role <= "pilot", active <= false),
  @(name <= "Ritsuko", age <= 30, role <= "scientist", active <= true)
]

// アクティブなスタッフの名前を取得します
staff => Filter[_, _ s = s.active]() => Map[_, _ s = s.name]() => activeNames
// activeNames: @["Asuka", "Shinji", "Ritsuko"]

// 平均年齢を計算します
staff => Map[_, _ s = s.age]() => ages
Fold[ages, 0, _ acc a = acc + a]() ]=> totalAge
Div[totalAge, ages.length()]() ]=> avgAge
// avgAge: 18
```

### 売上データの集計

```taida
orders <= @[
  @(product <= "A", quantity <= 5, price <= 100),
  @(product <= "B", quantity <= 3, price <= 200),
  @(product <= "A", quantity <= 2, price <= 100)
]

// 各注文の小計を計算し、合計します
orders => Map[_, _ o = o.quantity * o.price]() => subtotals
Sum[subtotals]() ]=> totalRevenue
// totalRevenue: 1300

// 商品Aだけの合計
orders => Filter[_, _ o = o.product == "A"]() => Map[_, _ o = o.quantity * o.price]() => aSubtotals
Sum[aSubtotals]() ]=> aRevenue
// aRevenue: 700
```

---

## 型シグネチャ一覧

| モールド | `[]` 必須引数 | `()` オプション | 戻り値 |
|---------|-------------|----------------|--------|
| `Map[list, fn]()` | list, fn | - | @[U] |
| `Filter[list, fn]()` | list, fn | - | @[T] |
| `Fold[list, init, fn]()` | list, init, fn | - | A |
| `Foldr[list, init, fn]()` | list, init, fn | - | A |
| `Take[list, n]()` | list, n | - | @[T] |
| `Drop[list, n]()` | list, n | - | @[T] |
| `TakeWhile[list, fn]()` | list, fn | - | @[T] |
| `DropWhile[list, fn]()` | list, fn | - | @[T] |
| `Append[list, val]()` | list, val | - | @[T] |
| `Prepend[list, val]()` | list, val | - | @[T] |
| `Concat[list, other]()` | list, other | - | @[T] |
| `Sort[list]()` | list | reverse, by | @[T] |
| `Reverse[list]()` | list | - | @[T] |
| `Unique[list]()` | list | by | @[T] |
| `Flatten[list]()` | list | - | @[U] |
| `Join[list, sep]()` | list, sep | - | Str |
| `Sum[list]()` | list | - | Num |
| `Find[list, fn]()` | list, fn | - | Lax[T] |
| `FindIndex[list, fn]()` | list, fn | - | Int |
| `Count[list, fn]()` | list, fn | - | Int |
| `Zip[list, other]()` | list, other | - | @[BuchiPack] |
| `Enumerate[list]()` | list | - | @[BuchiPack] |

---

## まとめ

| 分類 | API | 返す値 |
|------|-----|--------|
| **状態チェック** | `length()`, `isEmpty()`, `contains()`, `indexOf()`, `lastIndexOf()` | Int / Bool |
| **安全アクセス** | `first()`, `last()`, `get()`, `max()`, `min()` | Lax[T] |
| **述語** | `any()`, `all()`, `none()` | Bool |
| **操作** | `Map`, `Filter`, `Fold`, `Sort`, `Reverse`, etc. | モールド |

メソッドは「聞くだけ」、モールドは「作り変える」。ゴリラリテラルは終了。この使い分けが Taida のリスト操作の基本です。

詳しいモールドの仕様は [クラスライク型リファレンス (操作モールド中心)](../reference/class_like_types.md) を、メソッドの仕様は [標準メソッドリファレンス](../reference/standard_methods.md) を参照してください。
