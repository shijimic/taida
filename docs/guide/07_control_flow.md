# 制御フロー

> **PHILOSOPHY.md -- I.** 深く考えずに適当にぶちこんでけ

Taida には if/else も switch も三項演算子もありません。あるのは `|` と `|>` だけです。

---

## 条件分岐 `|` `|>`

### 基本構文

```taida
| 条件1 |> 値1
| 条件2 |> 値2
| _ |> デフォルト値
```

`_` はプレースホルダで、「それ以外の全て」を意味します。

### 変数への代入

条件分岐は式なので、結果を直接変数に代入できます。

```taida
score <= 85

grade <=
  | score >= 90 |> "A"
  | score >= 80 |> "B"
  | score >= 70 |> "C"
  | score >= 60 |> "D"
  | _ |> "F"
// grade: "B"
```

### 1行で書く

```taida
sign <= | x >= 0 |> "positive" | _ |> "negative"
```

### 複合条件

`&&` や `||` で条件を組み合わせることができます。

```taida
canDrive <=
  | age >= 18 && hasLicense |> true
  | _ |> false
```

---

## パターンマッチング的な条件分岐

### 値のパターンマッチ

```taida
status <= "success"

message <=
  | status == "success" |> "Operation completed"
  | status == "error" |> "Operation failed"
  | status == "pending" |> "Operation in progress"
  | _ |> "Unknown status"
```

### ぶちパックのフィールドによるマッチ

```taida
response <= @(code <= 200, data <= "OK")

result <=
  | response.code == 200 |> "Success: " + response.data
  | response.code == 404 |> "Not found"
  | response.code >= 500 |> "Server error"
  | _ |> "Unknown response"
```

### ネストした条件

条件分岐はネストできます。

```taida
staff <= @(age <= 29, role <= "commander", active <= true)

accessLevel <=
  | staff.role == "commander" |>
    | staff.active |> "full"
    | _ |> "readonly"
  | staff.role == "operator" |>
    | staff.age >= 18 |> "standard"
    | _ |> "limited"
  | _ |> "none"
// accessLevel: "full"
```

---

## ゴリラリテラル `><` との組み合わせ

条件分岐の先に `><` を置くと、プログラムは即座に終了します。

`><` はゴリラの顔文字です。日本語の顔文字で「ムキーッ」を意味する怒りの表情が、そのまま即時終了リテラルになりました。実行中にゴリラが出現すれば、プログラムは終了します。

```taida
processOrder order =
  | order.total < 0 |> ><      // 負の金額にゴリラが出現します。プログラムは終了します。
  | order.items.isEmpty() |> ><  // 商品なしにもゴリラが出現します。
  | _ |> submitOrder(order)
=> :OrderResult
```

条件分岐の中でゴリラが現れるということは、「ここに来たらもう終わりです」というプログラマの意思表明です。

### exit() との対比

| | `exit()` | `><` |
|--|----------|------|
| **性質** | 手続き（関数呼び出し） | 宣言（リテラル） |
| **戻り値の型** | Never / void / ! | ゴリラに型はありません |
| **エラーコード** | 0? 1? 42? 選ぶ必要があります | ゴリラにコードはありません |
| **クリーンアップ** | atexit? defer? finally? | ゴリラは片付けません |
| **import** | 必要です | 不要です。`><` は言語そのものです |
| **文字数** | 11~22文字 | **2文字** |

### なぜ `><` は見間違えないのか

`>` と `<` は比較演算子です。これを**逆向きにぶつける**と `><` になります。二つの矢印が正面衝突しています。クラッシュです。概念がそのまま記号になっています。`>` でもなく `<` でもない `><` だけが「終わり」を意味するため、他のどの演算子とも混同しません。

### AI との相性

AST 上で `><` は `Expr::Gorilla` として表現されます。AI にとっては以下のような利点があります。

- `><` がある分岐は「到達不能コード」として即座に判定できます
- 「ここは来てはいけない」を `><` の1トークンで表現できます
- 構造的イントロスペクションにおいて、終端ノードとして明確にマーキングされます

```taida
// AI が生成するコード例
validateInput input =
  | input.type != "expected" |> ><  // 不正な入力にはゴリラが出現します
  | input.value < 0 |> ><          // 負の値にもゴリラが出現します
  | _ |> processInput(input)
=> :Output
```

AI が `><` を使うとき、そこには曖昧さがありません。「この分岐に入ったらプログラムは終わる」ということ以上でもそれ以下でもありません。

---

## ガード節パターン

関数内での早期リターンとして条件分岐を使えます。異常系を先に弾き、正常系を最後に書くパターンです。

```taida
processStaff staff =
  | !staff.active |> @(success <= false, message <= "Inactive staff")
  | staff.age < 18 |> @(success <= false, message <= "Under age")
  | _ |>
    deployed <= assignMission(staff)
    @(success <= true, message <= "Assigned: " + deployed.name)
=> :@(success: Bool, message: Str)
```

`><` と組み合わせると、致命的な前提条件の違反を表現できます。

```taida
launchEva pilot eva =
  | !pilot.active |> ><           // 非アクティブパイロットは起動不可です
  | eva.power < 0 |> ><           // 電力不正は致命的です
  | pilot.syncRate < 10 |>
    @(success <= false, reason <= "Sync rate too low")
  | _ |>
    result <= activateEva(pilot, eva)
    @(success <= true, reason <= "")
=> :@(success: Bool, reason: Str)
```

---

## 再帰ループ

Taida には for や while のようなループ構文がありません。繰り返し処理は再帰で表現可能です。末尾再帰は自動的に最適化されるため、スタックオーバーフローを心配する必要はありません。

### 基本パターン: アキュムレータ

```taida
// 1 から n までの合計を計算します
sumTo n: Int =
  sumToTail(n, 0)
=> :Int

sumToTail n: Int acc: Int =
  | n < 1 |> acc
  | _ |> sumToTail(n - 1, acc + n)  // 末尾位置: 最適化されます
=> :Int

result <= sumTo(1000000)  // スタックオーバーフローしません
```

### 階乗

```taida
factorial n: Int =
  factorialTail n 1
=> :Int

factorialTail n: Int acc: Int =
  | n < 2 |> acc
  | _ |> factorialTail(n - 1, acc * n)
=> :Int
```

### フィボナッチ数列

```taida
fibonacci n: Int =
  fibTail n 0 1
=> :Int

fibTail n: Int a: Int b: Int =
  | n == 0 |> a
  | n == 1 |> b
  | _ |> fibTail(n - 1, b, a + b)
=> :Int
```

### リスト処理での再帰

多くのリスト処理はモールドで十分ですが、複雑なロジックには再帰が適しています。

```taida
// リストから条件を満たす最初の要素を見つけ、加工して返します
findAndProcess items: @[Item] =
  | items.isEmpty() |> @(found <= false, result <= "")
  | _ |>
    items.first() ]=> current
    | current.priority > 5 |>
      processed <= transform(current)
      @(found <= true, result <= processed.name)
    | _ |>
      Drop[items, 1]() ]=> rest
      findAndProcess(rest)  // 末尾再帰
=> :@(found: Bool, result: Str)
```

末尾再帰最適化の詳細は [末尾再帰最適化リファレンス](../reference/tail_recursion.md) を参照してください。

---

## 純粋式の原則（`| |>` アーム本体の構文境界）

> **PHILOSOPHY.md -- I.** 深く考えずに適当にぶちこんでけ

`| cond |> ...` のアーム本体は **純粋な式** です。副作用文（結果を捨てる関数呼び出しや `=> _` パイプライン）をアームの途中に埋め込むことはできません。

### アーム本体に書ける要素

アーム本体は「**任意個の let バインディング** → **最終結果式 1 つ**」の形を取ります。

許可される非終端文（let バインディング）:

- `name <= expr` — 値バインディング
- `expr ]=> name` — 前方アンモールド（Lax / Option 解凍）
- `name <=[ expr` — 後方アンモールド

最終行は **値を生む式** でなければなりません。

```taida
// OK: let バインディング → 最終式
classify n =
  | n > 0 |>
    doubled <= n * 2
    doubled + 1      // 最終式
  | _ |>
    zeroed <= 0
    zeroed - 1       // 最終式
=> :Int
```

### アーム本体に書けない要素

以下はいずれも `[E1616]` でコンパイルエラーになります。副作用文の混入を禁止する境界です。

- 非終端位置の裸の関数呼び出し文（`stdout("x")` のみが 1 行として置かれる）
- `expr => _var` パターンでの値の捨て（哲学 I「暗黙の排除」に反するため）
- アーム本体内での関数定義 / 型定義 / モールド定義 / `|==` エラー天井 / `>>>` import / `<<<` export
- 最終行が let バインディングで終わる（最終式がない）

```taida
// NG: FB-17 の再現ケース — アーム内に副作用文が紛れ込む
validateProjectRoot =
  | Exists["./"]() == false |>
    fail("error: current directory does not exist")
    writeFile(".hk_write_check", "test") => _wr   // [E1616]: 副作用文は禁止
    RemoveFile[".hk_write_check"]() => _rm
  | _ |>
    "ok"
=> :Str
```

### Migration: 従来コードの書き換え

副作用を伴う前処理が必要な場合は **アーム本体の外に出す** か、ヘルパ関数 / `If` モールドを使います。

```taida
// Before (NG): アーム内で副作用を起こしていた
run =
  | needsInit |>
    setup() => _s
    runApp()
  | _ |> runApp()
=> :Int

// After (OK): 副作用は関数として切り出し、アーム本体は純粋式に
run =
  setup_if_needed() => _
  runApp()
=> :Int

setup_if_needed =
  | needsInit |> setup()
  | _ |> 0
=> :Int
```

短い 2 分岐なら `If` モールドが簡潔です。

```taida
result <= If[cond, doSomething(), 0]()
```

### なぜこの制約なのか

Taida では `| cond |> value` は **式** なので、分岐の途中に状態遷移を許すと「式」ではなくなり、プログラムの構造的な意味が揺らぎます（哲学 IV「書くのも読むのも AI、眺めるのが人間」の前提が崩れます）。純粋式の原則により、条件分岐は常にグラフ上で単一ノードとして扱え、構造的イントロスペクションが確定的に働きます。

---

## 式ベース

すべての条件分岐は値を返します。文ではなく式です。

```taida
// 条件分岐の結果を直接使えます
message <= "Status: " + (| active |> "ON" | _ |> "OFF")
```

すべての分岐は同じ型を返す必要があります。型チェッカーがこれを保証します。

---

## エラー処理との組み合わせ

条件分岐は [エラー処理](08_error_handling.md) の `|==` エラー天井や `throw` と組み合わせて使えます。

```taida
Error => RequestError = @(detail: Str)

processRequest request =
  |== error: RequestError =
    @(success <= false, error <= error.message)
  => :@(success: Bool, error: Str)

  | request.body == "" |> RequestError(type <= "RequestError", message <= "Empty body", detail <= "body").throw()
  | request.method != "POST" |> RequestError(type <= "RequestError", message <= "Invalid method", detail <= request.method).throw()
  | _ |>
    result <= process(request.body)
    @(success <= true, error <= "")
=> :@(success: Bool, error: Str)
```

---

## If モールド -- 2 分岐の条件式

2 分岐だけの条件分岐には `If[condition, then, else]()` モールドが使えます。`| |>` の簡潔な代替です。

```taida
// If モールド（2 分岐向き）
result <= If[x > 0, "positive", "negative"]()

// | |> 構文（複数分岐向き）
grade <=
  | score >= 90 |> "A"
  | score >= 80 |> "B"
  | _ |> "C"
```

パイプラインで `_` を使って前段の値を参照できます。

```taida
// clamp: 100 を超えたら 100 に制限
150 => If[_ > 100, 100, _]() => clamped   // 100

// 分類
5 => If[_ > 3, "big", "small"]() => label  // "big"
```

非選択 branch は評価されません（short-circuit）。

---

## 型比較モールド -- TypeIs / TypeExtends

型の判定にもモールドを使います。

```taida
// 実行時の型チェック
TypeIs[42, :Int]()           // true
TypeIs["hello", :Str]()     // true

// Enum variant 判定
Enum => Status = :Ok :Fail
TypeIs[x, Status:Ok]()      // true（x が Status:Ok の場合）

// 型の継承関係チェック
TypeExtends[:Int, :Num]()   // true
TypeExtends[:Dog, :Animal]()   // true（Dog が Animal を継承している場合）
```

詳細は「モールディング型」ガイドの型比較モールドセクションを参照してください。

---

## まとめ

| 構文 | 用途 |
|------|------|
| `\| cond \|> val` | 条件分岐（複数分岐向き） |
| `\| _ \|> val` | デフォルトケース |
| `\| cond \|> ><` | ゴリラが出現し、プログラムは即時終了します |
| `If[cond, then, else]()` | 2 分岐の条件式 |
| `TypeIs[val, :Type]()` | 実行時の型チェック |
| `TypeExtends[:A, :B]()` | 型の継承関係チェック |
| ネスト | 複雑な条件 |
| 1行 | 三項演算子の代替 |
| ガード節 | 異常系を先に弾くパターン |
| 再帰 | ループの代替（末尾再帰は自動最適化） |
