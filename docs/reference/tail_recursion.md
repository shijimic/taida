# 末尾再帰最適化 (Tail Recursion Optimization)

## 概要

末尾再帰最適化（TCO: Tail Call Optimization）は、関数の最後の操作が自分自身の呼び出しである場合に、スタックを消費せずに実行する最適化です。

## 設計原則

- **自動検出**: アノテーション不要（PHILOSOPHY.md — I.）
- **保証された最適化**: 末尾位置にある再帰呼び出しは必ず最適化される
- **エラーなし**: 末尾再帰でない場合もコンパイルエラーにはならない

---

## 末尾位置とは

「末尾位置」とは、その式の評価結果がそのまま関数の戻り値になる位置のことです。

### 末尾位置である例

```taida
// 関数の最後の式
factorial n: Int acc: Int =
  | n < 1 |> acc
  | _ |> factorial(n - 1, acc * n)  // ← 末尾位置
=> :Int
```

### 末尾位置でない例

```taida
// 再帰呼び出しの後に演算がある
factorial n: Int =
  | n < 1 |> 1
  | _ |> n * factorial(n - 1)  // ← 末尾位置ではない（乗算が後に続く）
=> :Int
```

---

## 末尾位置の判定ルール

### 1. 関数本体の最後の式

```taida
// 末尾位置
foo x: Int =
  bar(x)  // ← 関数本体の最後 = 末尾位置
=> :Int
```

### 2. 条件分岐の各分岐の最後

```taida
// 両方の分岐が末尾位置
factorial n: Int acc: Int =
  | n < 1 |> acc               // ← 末尾位置
  | _ |> factorial(n - 1, acc * n)  // ← 末尾位置
=> :Int
```

### 3. ネストした条件分岐の最後

```taida
classify x: Int =
  | x < 0 |>
    | x < -100 |> handleVeryNegative(x)  // ← 末尾位置
    | _ |> handleNegative(x)             // ← 末尾位置
  | x == 0 |> handleZero(x)              // ← 末尾位置
  | _ |>
    | x > 100 |> handleVeryPositive(x)   // ← 末尾位置
    | _ |> handlePositive(x)             // ← 末尾位置
=> :Int
```

### 4. エラーシーリング内の最後

```taida
process x: Int =
  |== error: Error =
    defaultValue  // ← 末尾位置（エラー分岐）
  => :Int

  calculate(x)    // ← 末尾位置（正常分岐）
=> :Int
```

---

## 末尾位置でないパターン

### 1. 演算子の引数

```taida
// NOT 末尾位置: 乗算の引数
badFactorial n: Int =
  | n < 1 |> 1
  | _ |> n * badFactorial(n - 1)  // × 乗算が後続
=> :Int
```

### 2. 関数呼び出しの引数

```taida
// NOT 末尾位置: wrapの引数
foo x: Int =
  wrap(foo(x - 1))  // × wrapの呼び出しが後続
=> :Int
```

### 3. 変数束縛の右辺

```taida
// NOT 末尾位置: 後続の式がある
foo x: Int =
  result <= bar(x)  // × 後に result の返却がある
  result
=> :Int
```

### 4. unmold/]=>の右辺

```taida
// NOT 末尾位置
foo x: Int =
  asyncOp(x) ]=> result  // × 後続処理がある
  result
=> :Int
```

---

## アキュムレータパターン

末尾再帰を実現する典型的なパターンは「アキュムレータ」を使う方法です。

### Before: 末尾再帰でない

```taida
// スタックが n 回積まれる
factorial n: Int =
  | n < 1 |> 1
  | _ |> n * factorial(n - 1)
=> :Int
```

### After: 末尾再帰

```taida
// スタックは1回のみ（最適化される）
factorial n: Int =
  factorialTail n 1
=> :Int

factorialTail n: Int acc: Int =
  | n < 1 |> acc
  | _ |> factorialTail(n - 1, acc * n)  // 末尾位置
=> :Int
```

---

## 実用例

### リストの合計

```taida
// 末尾再帰版
sum list: @[Int] =
  sumTail list 0
=> :Int

sumTail list: @[Int] acc: Int =
  | list.isEmpty() |> acc
  | _ |>
    list.first() ]=> head
    Drop[list, 1]() ]=> rest
    sumTail(rest, acc + head)
=> :Int
```

### リストの反転

```taida
// 末尾再帰版
reverse list: @[Int] =
  reverseTail list @[]
=> :@[Int]

reverseTail list: @[Int] acc: @[Int] =
  | list.isEmpty() |> acc
  | _ |>
    list.first() ]=> head
    Drop[list, 1]() ]=> rest
    Prepend[acc, head]() ]=> newAcc
    reverseTail(rest, newAcc)
=> :@[Int]
```

### フィボナッチ数列

```taida
// 末尾再帰版
fibonacci n: Int =
  fibTail n 0 1
=> :Int

fibTail n: Int a: Int b: Int =
  | n == 0 |> a
  | n == 1 |> b
  | _ |> fibTail(n - 1, b, a + b)
=> :Int
```

### 二分探索

```taida
binarySearch list: @[Int] target: Int =
  searchTail list target 0 (list.length() - 1)
=> :Int

searchTail list: @[Int] target: Int low: Int high: Int =
  | low > high |> 0 - 1
  | _ |>
    Div[low + high, 2]() ]=> mid
    list.get(mid) ]=> value
    | value == target |> mid
    | value < target |> searchTail(list, target, mid + 1, high)
    | _ |> searchTail(list, target, low, mid - 1)
=> :Int
```

### 文字列の繰り返し

```taida
repeat str: Str n: Int =
  repeatTail str n ""
=> :Str

repeatTail str: Str n: Int acc: Str =
  | n < 1 |> acc
  | _ |> repeatTail(str, n - 1, acc + str)
=> :Str
```

---

## 相互再帰

相互再帰（2つ以上の関数が互いを末尾位置で呼び出す）も末尾呼び出し最適化の対象です。コンパイラが関数間の呼び出しグラフを解析し、相互再帰グループを自動検出します。

```taida
// isEven と isOdd が互いを末尾位置で呼び出す → TCO が適用される
isEven n: Int =
  | n == 0 |> 1
  | _ |> isOdd(n - 1)
=> :Int

isOdd n: Int =
  | n == 0 |> 0
  | _ |> isEven(n - 1)
=> :Int

// 100,000 回の相互再帰でもスタックオーバーフローしない
stdout(isEven(100000))
```

### 相互再帰の条件

相互再帰が最適化されるための条件は、直接再帰と同じです:

1. **末尾位置**: 相手関数の呼び出しが末尾位置にあること
2. **自動検出**: アノテーション不要。コンパイラが呼び出しグラフから相互再帰グループを検出する
3. **3つ以上の関数**: 2関数に限らず、A->B->C->A のような3つ以上の関数の循環も最適化される

### バックエンド対応状況

| バックエンド | 直接再帰 TCO | 相互再帰 TCO |
|-------------|:-----------:|:-----------:|
| Interpreter | OK | OK |
| JS          | OK | OK |
| Native      | OK | 通常呼び出し |

Native バックエンドでは、相互再帰は通常の関数呼び出しとして実行されます。深い相互再帰ではスタックオーバーフローの可能性があるため、Interpreter または JS バックエンドの使用を推奨します。

### 非末尾の相互再帰はコンパイルエラー

非末尾位置での相互再帰は、実行時に必ずスタックオーバーフロー（`Maximum call depth (256) exceeded`）を起こすため、`@c.12.rc3` 以降はコンパイル時に拒否されます。

```taida
// NG: wrap(emptyNodes(n)) の emptyNodes 呼び出しは非末尾位置
emptyDomNode n =
  wrap(emptyNodes(n))

emptyNodes n =
  emptyDomNode(n)
```

```
error [E1614]: Mutual recursion in non-tail position: emptyDomNode -> emptyNodes -> emptyDomNode.
  The non-tail call 'emptyNodes' inside 'emptyDomNode' will overflow the stack at runtime.
  Hint: rewrite the recursive call so it is the last operation in the function body,
  or convert to an accumulator-passing style (see docs/reference/tail_recursion.md).
```

#### 検出ロジック

1. `taida check` / `taida build` / `taida verify` は、プログラムから**コールグラフ**（`GraphView::Call`）を抽出する。
2. DFS による `query::find_cycles` で関数間の循環を列挙する。
3. サイクルに含まれる 2 関数以上のノードごとに、**呼び出し元の AST を走査し末尾位置判定を行う**（`src/graph/tail_pos.rs::collect_call_sites`）。末尾位置の規則は上記「末尾位置とは」と同一。
4. サイクル上のいずれかのエッジに**非末尾**呼び出しが存在すれば `[E1614]` を発行してコンパイルを停止する。
5. すべてのエッジが末尾位置であれば、検査はパスする（Interpreter / JS ではトランポリン、Native では通常呼び出しで動作）。

直接再帰（`count → count`）は「相互」ではないため `mutual-recursion` チェックの対象外。従来どおりランタイムで処理されます。

#### エラーが出たら

1. 再帰呼び出しを**本体の最後**に移動する（関数の引数位置 / 二項演算 / `@(...)` フィールド式から外に出す）。
2. 中間結果はアキュムレータ引数として渡す（本書「アキュムレータパターン」節を参照）。
3. どうしても非末尾にしたい場合は、再帰ではなく通常のループ的な繰り返しに置き換える（`@[1..n]` を `.reduce(_)` するなど）。

---

## エラーシーリングとの相互作用

エラーシーリング内での末尾再帰も最適化されます。

```taida
processItems items: @[Item] =
  |== error: Error =
    @[]  // エラー時は空リスト
  => :@[Result]

  processItemsTail items @[]
=> :@[Result]

processItemsTail items: @[Item] acc: @[Result] =
  | items.isEmpty() |> acc
  | _ |>
    items.first() ]=> head
    result <= processOne(head)
    Drop[items, 1]() ]=> rest
    Append[acc, result]() ]=> newAcc
    processItemsTail(rest, newAcc)  // 末尾位置
=> :@[Result]
```

---

## 継続渡しスタイル (CPS)

より複雑なケースでは、継続渡しスタイルを使って末尾再帰に変換できます。

```taida
// 継続渡しスタイルでの階乗
factorialCPS n: Int cont: :Int => :Int =
  | n < 1 |> cont(1)
  | _ |> factorialCPS(n - 1, _ result = cont(n * result))
=> :Int

// 使用
factorial10 <= factorialCPS(10, _ x = x)
// factorial10: 3628800
```

---

## パフォーマンス考慮

### 最適化される場合

- **スタック消費**: O(1)（定数）
- **関数呼び出しオーバーヘッド**: 最小化（ジャンプに変換）

### 最適化されない場合

- **スタック消費**: O(n)（再帰の深さに比例）
- **スタックオーバーフロー**: 深い再帰で発生する可能性

### 推奨

大量のデータを処理する再帰関数では、末尾再帰パターンを使用することを推奨します。

```taida
// 良い例: 末尾再帰（大きなリストでも安全）
sumTail list @[]

// 注意: 末尾再帰でない（大きなリストでスタックオーバーフローの可能性）
sum list
```

---

## チェックリスト

末尾再帰になっているか確認するためのチェックリスト：

1. **再帰呼び出しは最後の操作か？**
   - 再帰呼び出しの後に演算がないこと

2. **再帰呼び出しの結果をそのまま返しているか？**
   - 変数に代入していないこと
   - 別の関数に渡していないこと

3. **すべての分岐で末尾位置か？**
   - 条件分岐のすべてのパスを確認

4. **アキュムレータを使っているか？**
   - 中間結果を引数として渡しているか

---

## まとめ

末尾再帰最適化により：

1. **自動検出**: コンパイラが自動的に末尾位置を検出
2. **スタック効率**: 末尾再帰は定数スタックで実行
3. **アキュムレータパターン**: 末尾再帰への変換の標準的な方法
4. **安全な大量データ処理**: スタックオーバーフローを回避
5. **相互再帰**: Interpreter/JS バックエンドで自動検出・最適化
