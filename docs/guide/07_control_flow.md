# 制御フロー

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ

Taida には if/else も switch も三項演算子もありません。あるのは `|` と `|>` だけです。

---

## 条件分岐 `|` `|>`

### 基本構文

```taida fragment
| 条件1 |> 値1
| 条件2 |> 値2
| _ |> デフォルト値
```

`_` はプレースホルダで、「それ以外の全て」を意味します。

### 変数への代入

条件分岐は式なので、結果を直接変数に代入できます。

```taida
score <= 85

grade <= (
  | score >= 90 |> "A"
  | score >= 80 |> "B"
  | score >= 70 |> "C"
  | score >= 60 |> "D"
  | _ |> "F"
)
// grade: "B"
```

> **Note:** `<=` の右辺に **複数行** の `| cond |> body` 多アーム条件を
> 直接書くと [`E0303`](../reference/diagnostic_codes.md#e0303) で拒否されます。
> 上記のように丸括弧で包む（escape hatch）、`If[cond, then, else]()` を使う、
> またはヘルパ関数に抽出してください。単一行の多アームはそのまま書けます
> （下の「1行で書く」参照）。

### 1行で書く

```taida fragment
sign <= | x >= 0 |> "positive" | _ |> "negative"
```

### `| _ |>` を最後に置く

default arm (`| _ |>` または `| true |>`) は条件分岐に必須です。欠落した条件分岐は型チェッカーで `[E1524]` として reject されます。default arm を最後に置くと、上から順に評価する読み方がそのまま「いずれにも合致しなかったときの fallback」になり、読み手が混乱しません。実行は上から順なので、default arm を途中に置くと後続の arm が決して評価されない (dead code) ことに注意してください。

### 複合条件

`&&` や `||` で条件を組み合わせることができます。

```taida fragment
canDrive <= (
  | age >= 18 && hasLicense |> true
  | _ |> false
)
```

---

## パターンマッチング的な条件分岐

### 値のパターンマッチ

```taida
status <= "success"

message <= (
  | status == "success" |> "Operation completed"
  | status == "error" |> "Operation failed"
  | status == "pending" |> "Operation in progress"
  | _ |> "Unknown status"
)
```

### ぶちパックのフィールドによるマッチ

```taida
response <= @(code <= 200, data <= "OK")

result <= (
  | response.code == 200 |> "Success: " + response.data
  | response.code == 404 |> "Not found"
  | response.code >= 500 |> "Server error"
  | _ |> "Unknown response"
)
```

### ネストした条件

条件分岐はネストできます。

```taida
staff <= @(age <= 29, role <= "commander", active <= true)

accessLevel <= (
  | staff.role == "commander" |>
    | staff.active |> "full"
    | _ |> "readonly"
  | staff.role == "operator" |>
    | staff.age >= 18 |> "standard"
    | _ |> "limited"
  | _ |> "none"
)
// accessLevel: "full"
```

---

## ゴリラリテラル `><` との組み合わせ

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ

条件分岐の先に `><` を置くと、プログラムは即座に終了します。「**この分岐に到達したらもう終わり**」というプログラマの意思表明です。パターンマッチで到達不能を示すための主道リテラルとして使ってください。

```taida fragment
processOrder order =
  | order.total < 0 |> ><          // 負の金額にゴリラが出現します
  | order.items.isEmpty() |> ><    // 商品なしにもゴリラが出現します
  | _ |> submitOrder(order)
=> :OrderResult
```

`><` がある分岐は構造的イントロスペクションでも「到達不能コード」として即座に判定できます。

`exit(N)` は exit code を明示したい場合に使い、エラー経路で code を選ばないなら `><` を使ってください。`exit` の詳細は [`docs/api/prelude.md §6.1`](../api/prelude.md#61-exit) を参照してください。

---

## ガード節パターン

関数内での早期リターンとして条件分岐を使えます。異常系を先に弾き、正常系を最後に書くパターンです。

```taida fragment
processStaff staff =
  | !staff.active |> @(success <= false, message <= "Inactive staff")
  | staff.age < 18 |> @(success <= false, message <= "Under age")
  | _ |>
    deployed <= assignMission(staff)
    @(success <= true, message <= "Assigned: " + deployed.name)
=> :@(success: Bool, message: Str)
```

`><` と組み合わせると、致命的な前提条件の違反を表現できます。

```taida fragment
launchEva pilot eva =
  | !pilot.active |> ><           // 非アクティブパイロットは起動不可です
  | eva.power < 0 |> ><           // 電力不正は致命的です
  | pilot.sync_rate < 10 |>
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

```taida fragment
// リストから条件を満たす最初の要素を見つけ、加工して返します
findAndProcess items: @[Item] =
  | items.isEmpty() |> @(found <= false, result <= "")
  | _ |>
    items.first() >=> current
    | current.priority > 5 |>
      processed <= transform(current)
      @(found <= true, result <= processed.name)
    | _ |>
      Drop[items, 1]() >=> rest
      findAndProcess(rest)  // 末尾再帰
=> :@(found: Bool, result: Str)
```

末尾再帰最適化の詳細は [末尾再帰最適化リファレンス](../reference/tail_recursion.md) を参照してください。

---

## `| |>` アーム本体の構文境界

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ

`| cond |> ...` のアーム本体は **純粋な式** です。副作用文 (結果を捨てる関数呼び出しや `=> _var` パイプライン) をアームの途中に埋め込むことはできません。この制約はアーム本体に限定されます — 関数本体と `|==` エラー天井本体には適用されません。

### アーム本体に書ける要素

アーム本体は「**任意個の let バインディング** → **最終結果**」の形を取ります。

許可される非終端文 (let バインディング):

- `name <= expr` — 値バインディング
- `expr => name` — 単一方向パイプラインの末尾に書かれた値バインディング
- `expr >=> name` — 前方アンモールド (Lax / Option 解凍)
- `name <=< expr` — 後方アンモールド

**最終行** は次のいずれでも構いません。

- **値を生む式** — そのまま式の値がアームの結果になる
- **末尾バインディング** — 束縛された値がそのままアームの結果として返る

```taida fragment
// OK: let バインディング → 最終式
classify n =
  | n > 0 |>
    doubled <= n * 2
    doubled + 1      // 最終式
  | _ |>
    zeroed <= 0
    zeroed - 1       // 最終式
=> :Int

// OK: 末尾バインディングがアームの結果を兼ねる
classify n =
  | n > 0 |>
    doubled <= n * 2
    result <= doubled + 1   // 末尾バインド — アームの値は `result` (= doubled + 1)
  | _ |> 0
=> :Int
```

### アーム本体に書けない要素

以下はいずれも `[E1616]` でコンパイルエラーになります。副作用文の混入を禁止する境界です。

- 非終端位置の裸の関数呼び出し文 (`stdout("x")` のみが 1 行として置かれる)
- `expr => _var` パターンでの値の捨て (哲学 I「暗黙の排除」に反するため)。末尾位置でも `_var` への破棄は禁止
- アーム本体内での関数定義 / 型定義 / モールド定義 / `|==` エラー天井 / `>>>` import / `<<<` export

```taida
// NG: アーム内に副作用文が紛れ込む anti-pattern
validateProjectRoot =
  rootExists <= Exists["./"]()
  | rootExists.getOrDefault(false) == false |>
    fail("error: current directory does not exist")
    writeFile(".hk_write_check", "test") => _wr   // [E1616]: discard 破棄パターンは禁止
    RemoveFile[".hk_write_check"]() => _rm
  | _ |>
    "ok"
=> :Str
```

### 副作用を伴う前処理の書き方

副作用を伴う前処理が必要な場合は **アーム本体の外に出す** か、ヘルパ関数 / `If` モールドを使います。

```taida
// NG: アーム内で副作用を起こしている
run =
  | needsInit |>
    setup() => _s
    runApp()
  | _ |> runApp()
=> :Int

// OK: 副作用は関数として切り出し、アーム本体は純粋式に
run =
  setup_if_needed() => _
  runApp()
=> :Int

setup_if_needed =
  | needsInit |> setup()
  | _ |> 0
=> :Int
```

末尾バインド形を使うと、最終行に同名の識別子をもう 1 行置く冗長さを解消できます。

```taida fragment
// 冗長な書き方 — 末尾識別子を再掲
f x =
  | x > 0 |>
    doubled <= x * 2
    result <= doubled + 1
    result                 // 末尾に同じ名前をもう 1 行
  | _ |> 0
=> :Int

// 末尾バインドで完結
f x =
  | x > 0 |>
    doubled <= x * 2
    result <= doubled + 1  // 最終行の束縛値がそのままアームの値
  | _ |> 0
=> :Int
```

短い 2 分岐なら `If` モールドが簡潔です。

```taida fragment
result <= If[cond, doSomething(), 0]()
```

### なぜこの制約なのか

Taida では `| cond |> value` は **式** なので、分岐の途中に状態遷移を許すと「式」ではなくなり、プログラムの構造的な意味が揺らぎます (哲学 IV「書くのも読むのも AI、眺めるのが人間」の前提が崩れます)。純粋式の原則により、条件分岐は常にグラフ上で単一ノードとして扱え、構造的イントロスペクションが確定的に働きます。

末尾バインドは純粋式の原則と矛盾しません。構文上 let に見えますが、セマンティクスは「束縛＋その値を返す」という単一の純粋式と等価です。副作用文の混入禁止は引き続き維持されます。

---

## パイプラインの中間命名 (`=> name`)

純粋な `=>` 単一方向パイプラインでは、中間の `=> name` を **bind-and-forward** として書けます。束縛された値は同じ値のまま次段へ流れ、`name` は **その pipeline 文の残り区間** から参照可能です。

```taida
add x: Int y: Int = x + y => :Int

1 => add(3, _) => addResult => stdout(addResult.toString())
```

`addResult` のスコープはこの 1 文の残り区間だけです。次の文では見えません。
方向混在 (`=>` と `<=` の混在) および `expr => _tmp` 形の破棄は引き続き禁止です。

---

## 式ベース

すべての条件分岐は値を返します。文ではなく式です。

```taida fragment
// 条件分岐の結果を直接使えます
message <= "Status: " + (| active |> "ON" | _ |> "OFF")
```

すべての分岐は同じ型を返す必要があります。型チェッカーがこれを保証します。

---

## エラー処理との組み合わせ

条件分岐は [エラー処理](08_error_handling.md) の `|==` エラー天井や `throw` と組み合わせて使えます。

```taida fragment
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

## If モールド — 2 分岐の条件式

2 分岐だけの条件分岐には `If[condition, then, else]()` モールドが使えます。`| |>` の簡潔な代替です。

```taida fragment
// If モールド (2 分岐向き)
result <= If[x > 0, "positive", "negative"]()

// | |> 構文 (複数分岐向き、rhs 多行は丸括弧で包む)
grade <= (
  | score >= 90 |> "A"
  | score >= 80 |> "B"
  | _ |> "C"
)
```

パイプラインで `_` を使って前段の値を参照できます。同一ステップ内で `_` は同じ前段値に bind されるため、複数回参照できます (例: `_ > 100` と `_` の双方が `150` に bind)。

```taida
// clamp: 100 を超えたら 100 に制限
150 => If[_ > 100, 100, _]() => clamped   // 100

// 分類
5 => If[_ > 3, "big", "small"]() => label  // "big"
```

非選択 branch は評価されません (short-circuit)。`_` プレースホルダの全体仕様は [演算子リファレンスの `_` プレースホルダ節](../reference/operators.md#_-プレースホルダ) を参照してください。`If` を含む条件・型変換などのモールド全般のシグネチャは [`docs/api/prelude.md §7.8`](../api/prelude.md#78-条件モールド) を参照してください。

---

## まとめ

| 構文 | 用途 |
|------|------|
| `\| cond \|> val` | 条件分岐 (複数分岐向き) |
| `\| _ \|> val` | デフォルトケース (最後に置く) |
| `\| cond \|> ><` | ゴリラが出現し、プログラムは即時終了します |
| `If[cond, then, else]()` | 2 分岐の条件式 |
| ネスト | 複雑な条件 |
| 1 行 | 三項演算子の代替 |
| ガード節 | 異常系を先に弾くパターン |
| 再帰 | ループの代替 (末尾再帰は自動最適化) |
