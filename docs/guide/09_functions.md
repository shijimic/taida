# 関数

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ

関数は Taida Lang の基本構成要素です。定義も呼び出しもシンプルに、深く考えずに書けます。

---

## 関数定義の基本

### 引数ありの関数

```taida
// 単一引数
double x: Int =
  x * 2
=> :Int

// 複数引数
add x: Int y: Int =
  x + y
=> :Int
```

関数本体は `=` の後に記述し、戻り型は `=> :Type` で指定します。`=> :Type` のインデントは関数定義と同じレベルです。

### 複数行の処理

```taida
processData data: Str =
  cleaned <= Trim[data]()
  upper <= Upper[cleaned]()
  upper
=> :Str
```

Taida では、文字列や数値の操作はメソッドではなくモールドで行います。トリムは `Trim[str]()` に、大文字変換は `Upper[str]()` です。

> **末尾バインド短縮形**: 関数本体の末尾が `name <= expr` / `expr => name` / `expr ]=> name` / `name <=[ expr` であるとき、束縛された値がそのまま関数の戻り値になります。上の例は以下のように書けます:
>
> ```taida
> processData data: Str =
>   cleaned <= Trim[data]()
>   upper <= Upper[cleaned]()   // 末尾バインド — 戻り値は `upper`
> => :Str
> ```
>
> 末尾の冗長な `upper` 行は不要です。詳細は `docs/guide/07_control_flow.md` を参照してください。

### 引数なし関数

引数のない関数は `= ... => :Type` の形式で定義します。

```taida
getVersion =
  "1.0.0"
=> :Str

getDefaultPilot =
  @(name <= "Rei", age <= 14, role <= "Pilot")
=> :@(name: Str, age: Int, role: Str)
```

### generic function

generic function は「計算手続きの抽象化」に限定します。`Mold` がデータ/solidify/unmold の抽象化を担うのに対し、generic function は inference-first で関数本体の型を共有するための仕組みです。

```taida
id[T] x: T =
  x
=> :T

first[T] xs: @[T] =
  xs.get(0)
=> :Lax[T]

mapValue[T, U] value: T fn: T => :U =
  fn(value)
=> :U

id(1)                              // Int
first(@["a", "b"]).unmold()        // "a"
mapValue(1, _ x = x.toString())    // "1"
```

制約が必要な場合は `T <= :Num` のように書きます。

```taida
clampMin[T <= :Num] x: T min: T =
  | x < min |> min
  | _ |> x
=> :T
```

ジェネリック関数の現在の制約:

- 呼び出しは推論のみです。`id(1)` は使えますが、呼び出し側での明示的な型引数 `id[Int](1)` は未対応です
- 各型変数は、少なくとも1つの引数の型注釈に現れる必要があります
- 型変数名に組み込み型名や既存の具象型名（`Int`, `User`, `Box` など）は使えません
- 制約は `T <= :Num` のようにヘッダー側で宣言します

```taida
id[T] x: T =
  x
=> :T

// NG: T が引数の型注釈に現れないので推論不能
// make[T] =
//   1
// => :T

// NG: Int は具象型名なので型変数名に使えない
// id[Int] x: Int =
//   x
// => :Int
```

---

## 型注釈

### 引数の型注釈

引数には必ず型を指定します。

```taida
greet name: Str =
  "Hello, " + name + "!"
=> :Str

calculate x: Int y: Float =
  Float[x]() + y
=> :Float
```

### 戻り型（省略不可）

戻り型は `=> :Type` で指定します。**戻り型の省略はできません。**すべての関数は戻り型を明示する必要があります。

```taida
add x: Int y: Int =
  x + y
=> :Int

// NG: 戻り型がないとコンパイルエラーになります
// add x: Int y: Int =
//   x + y
```

### 複雑な戻り型

ぶちパック型やモールディング型も戻り型に使えます。

```taida
createProfile name: Str age: Int =
  @(name <= name, age <= age, active <= true)
=> :@(name: Str, age: Int, active: Bool)

safeDivide x: Int y: Int =
  Div[x, y]()
=> :Lax[Int]
```

### 引数のデフォルト値

関数引数のデフォルト値仕様は `docs/design/function_default_args.md` で固定されています。

要点:

1. 引数は `name: Type`（型注釈必須）
2. `name: Type <= expr` で明示デフォルトを指定可能
3. 明示指定がない引数は、型のデフォルト値を使う
4. 呼び出し時に省略できるのは末尾引数のみ
5. 引数過多はコンパイルエラー

```taida
sum3 a: Int b: Int <= 10 c: Int <= 20 =
  a + b + c
=> :Int

sum3(1, 2, 3)  // 6
sum3(1, 2)     // 23
sum3(1)        // 31
sum3()         // 30
```


### 関数型シグネチャ

関数型は `:引数型 => :戻り型` の形式です。

```taida
:Int => :Str        // Int から Str への関数
:Int :Int => :Int   // Int, Int から Int への関数
```

---

## ラムダ関数

関数名の位置に `_` を使うと無名関数（ラムダ）になります。

```taida
// 基本形
_ x = x * 2

// 複数引数
_ x y = x + y

// 無名引数無し
x <= 12
_ = x
```

### ラムダは単一式

ラムダの本体は**単一の式**でなければなりません。`=>`、`<=`、`]=>`、`<=[` のいずれかが本体に現れた時点で、名前付き関数を使う必要があります。

```taida
// OK: 単一式
_ x = x * 2
_ x y = x + y
_ = 42

// NG: 演算子が現れたら名前付き関数にすること
// _ x =
//   result <= x * 2
//   result
```

単一式であるため、戻り型は式の型から推論されます。名前付き関数のような `=> :Type` は不要であり、書くこともできません。

### リスト操作での使用

ラムダはリスト操作のモールドと組み合わせて使います。

```taida
numbers <= @[1, 2, 3, 4, 5]

// Map: 各要素を変換します
Map[numbers, _ x = x * 2]() ]=> doubled       // @[2, 4, 6, 8, 10]

// Filter: 条件に合う要素を抽出します
Filter[numbers, _ x = x > 3]() ]=> filtered   // @[4, 5]

// Fold: 畳み込みで集約します
Fold[numbers, 0, _ acc x = acc + x]() ]=> total  // 15
```

---

## パイプライン

### 順方向パイプライン `=>`

`=>` でデータを左から右へ流します。`_` が前のステップの結果を受け取ります。

```taida
// データが左から右へ流れます
"  hello world  " => Trim[_]() => Upper[_]() => result
// result: "HELLO WORLD"

// 数値の変換
5 => add(3, _) => multiply(2, _) => result  // 16
```

### `<=` による代入

`<=` は右の値を左の変数に代入します。

```taida
result <= Upper["hello"]()
// result: "HELLO"
```

> **注意**: `<=` のチェーンによる逆方向パイプライン（`result <= f() <= g() <= data`）は現在サポートされていません。順方向パイプライン `=>` を使用してください。

### パイプラインでのモールド使用

操作がモールドなので、パイプラインとの組み合わせが自然です。

```taida
// 文字列処理パイプライン
"  Asuka Langley  " => Trim[_]() => Upper[_]() => Split[_, " "]() => result
// result: @["ASUKA", "LANGLEY"]

// 数値処理パイプライン
@[3, 1, 4, 1, 5, 9, 2, 6] => Sort[_]() => Unique[_]() => Filter[_, _ x = x > 3]() => result
// result: @[4, 5, 6, 9]
```

### 単一方向制約

一つの文で `=>` と `<=` を混在させることはできません。

```taida
// OK: => のみ
data => Filter[_, _ x = x > 0]() => Map[_, _ x = x * 2]() => result

// OK（仕様上有効だが、現在のパーサーは <= チェーン未対応。将来対応予定）:
// result <= Map[_, _ x = x * 2]() <= Filter[_, _ x = x > 0]() <= data

// NG: 混在はコンパイルエラーになります
data => Filter[_, _ x = x > 0]() <= result  // コンパイルエラー
```

---

## 部分適用（空スロット）

関数呼び出しの引数位置を空にすると、その位置が「穴」になり、部分適用された関数が返ります。

```taida
// 第2引数を空にします（穴）
add5 <= add(5, )
result <= add5(3)  // 8

// 第1引数を空にします
doubleIt <= multiply(, 2)
result <= doubleIt(5)  // 10

// 複数の穴
both <= add(, )
result <= both(3, 5)  // 8
```

穴の有無で通常呼び出しと部分適用が区別されます。

```taida
add(5, 3)   // 通常呼び出し → 8
add(5, )    // 部分適用 → :Int => :Int の関数
add(5)      // 通常呼び出し（第2引数はデフォルト値）
```

> 注: 部分適用は通常の関数呼び出しのみに適用されます。TypeDef やモールドのインスタンス化には使用できません。

> 注: 引数に `_` を渡す構文 `add(5, _)` は使えません。空スロット `add(5, )` を使用してください。`taida check` で `[E1502]` として拒否されます。

> 注: 関数オーバーロード（同じ名前の関数を異なる引数パターンで複数定義すること）は Taida Lang では禁止されています。`taida check` で `[E1501]` として拒否されます。

---

## 末尾再帰最適化

Taida は末尾再帰を自動検出し、トランポリン方式で最適化します。アノテーションは不要です。スタックオーバーフローを気にせずに再帰が書けます。

### 基本パターン: アキュムレータ

```taida
factorial n: Int acc: Int =
  | n < 2 |> acc
  | _ |> factorial(n - 1, acc * n)
=> :Int

result <= factorial(10000, 1)  // スタックオーバーフローしません
```

### フィボナッチ数列

```taida
fibonacci n: Int a: Int b: Int =
  | n == 0 |> a
  | _ |> fibonacci(n - 1, b, a + b)
=> :Int

fib10 <= fibonacci(10, 0, 1)  // 55
```

### エラー天井内の末尾位置

エラー天井 `|==` 内での末尾再帰も最適化されます。

```taida
retryLoop maxRetries: Int attempt: Int =
  |== error: Error =
    | attempt < maxRetries |> retryLoop(maxRetries, attempt + 1)
    | _ |> "Failed after retries"
  => :Str

  riskyOperation()
=> :Str
```

### 末尾位置の判定

以下の位置にある再帰呼び出しが末尾再帰として最適化されます。

- 関数本体の最後の式
- 条件分岐 `| ... |>` の各分岐の最後
- エラー天井 `|==` 内の最後

再帰呼び出しの後に演算がある場合は末尾位置ではありません。

```taida
// OK: 末尾位置（最適化されます）
| _ |> factorial(n - 1, acc * n)

// NG: 末尾位置ではない（乗算が後続するため最適化されません）
| _ |> n * factorial(n - 1)
```

### 相互再帰

2つ以上の関数が互いを末尾位置で呼び出す相互再帰も、自動的に最適化されます。

```taida
isEven n: Int =
  | n == 0 |> 1
  | _ |> isOdd(n - 1)
=> :Int

isOdd n: Int =
  | n == 0 |> 0
  | _ |> isEven(n - 1)
=> :Int

stdout(isEven(100000))  // スタックオーバーフローしません
```

詳しくは [末尾再帰最適化リファレンス](../reference/tail_recursion.md) を参照してください。

---

## まとめ

| 概念 | 構文 |
|------|------|
| 関数定義 | `name args: Type = body => :ReturnType` |
| 引数なし関数 | `name = body => :ReturnType` |
| ラムダ | `_ x = expr` |
| 順方向パイプライン | `data => f(_) => g(_) => result` |
| 逆方向パイプライン | `result <= g(_) <= f(_) <= data` **(将来対応予定)** |
| 部分適用 | `add(5, )` 空スロットで穴を指定 |
| 末尾再帰 | アキュムレータパターンで自動最適化 |

次のガイド: [モジュールシステム](10_modules.md)
