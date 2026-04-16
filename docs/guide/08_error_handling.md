# エラー処理

> **PHILOSOPHY.md -- I.** 深く考えずに適当にぶちこんでけ

Taida には try-catch がありません。エラー処理は2つの仕組みで成り立っています。

1. **Lax による安全な操作** -- よくある失敗（ゼロ除算、範囲外アクセスなど）はプログラムを止めず、デフォルト値にフォールバックします
2. **throw と |== によるカスタムエラー** -- ビジネスロジック上の致命的なエラーは明示的に投げ、明示的にキャッチします

キャッチされなかったエラーは**ゴリラ天井**が最後に止めます。

---

## Taida のエラーモデル

従来の言語では、ゼロ除算も範囲外アクセスもビジネスエラーも、すべてが例外として同じ仕組みで処理されていました。Taida はこれを2つの層に分離しています。

| 層 | 失敗の種類 | 処理方法 | プログラムは止まるか |
|---|-----------|----------|-------------------|
| **Lax 層** | ゼロ除算、範囲外アクセス、型変換失敗 | デフォルト値にフォールバック | 止まりません |
| **throw 層** | カスタムエラー（ビジネスロジック） | `\|==` でキャッチ、またはゴリラ天井 | キャッチしなければ止まります |
| **Gorillax 層** | 外部操作の失敗（npm 等） | ゴリラ（即終了）、または `.relax()` で throw に変換 | 止まります（relax すればキャッチ可能） |

---

## Lax による安全な操作

Lax[T] は「操作が失敗しても必ず値を返す」モールド型です。`hasValue` フィールドで成功/失敗を判別でき、失敗時は型 T のデフォルト値にフォールバックします。

### 除算と剰余

Taida に `/` 演算子と `%` 演算子はありません。除算と剰余は `Div[x, y]()` と `Mod[x, y]()` モールドで行います。これらは Lax を返すため、ゼロ除算でもプログラムは停止しません。

```taida
// 正常な除算
Div[10, 3]() ]=> result   // 3
Mod[10, 3]() ]=> result   // 1

// ゼロ除算 -- プログラムは停止しません
Div[10, 0]() ]=> result   // 0 (Int のデフォルト値)
Mod[10, 0]() ]=> result   // 0

// hasValue で判別できます
Div[10, 3]().hasValue   // true
Div[10, 0]().hasValue   // false
```

### リストの安全アクセス

`get()`、`first()`、`last()`、`max()`、`min()` はすべて Lax を返します。空リストや範囲外アクセスでもプログラムは停止しません。

```taida
items <= @[10, 20, 30]

// 正常なアクセス
items.get(1) ]=> val      // 20
items.first() ]=> val     // 10
items.last() ]=> val      // 30
items.max() ]=> val       // 30
items.min() ]=> val       // 10

// 範囲外・空リスト -- プログラムは停止しません
items.get(100) ]=> val    // 0 (デフォルト値)
empty: @[Int] <= @[]
empty.first() ]=> val     // 0
empty.max() ]=> val       // 0
```

### 型変換

型変換モールド `Int[x]()`、`Float[x]()`、`Str[x]()`、`Bool[x]()` も Lax を返します。

```taida
// 正常な変換
Int["123"]() ]=> num     // 123
Float["3.14"]() ]=> val  // 3.14

// 変換失敗 -- プログラムは停止しません
Int["abc"]() ]=> num     // 0 (デフォルト値)
Int["abc"]().hasValue    // false
```

### Lax の活用パターン

#### hasValue による分岐

```taida
userInput <= "42"
Int[userInput]() => parsed

| parsed.hasValue |>
  stdout("Parsed: " + parsed.unmold().toString())
| _ |>
  stdout("Invalid input")
```

#### getOrDefault でカスタムデフォルト値

```taida
empty: @[Int] <= @[]
Div[total, count]().getOrDefault(0)       // ゼロ除算時は 0
empty.first().getOrDefault(99)            // 空リスト時は 99
Int["abc"]().getOrDefault(-1)             // 変換失敗時は -1
```

#### map による変換チェーン

```taida
// 成功時のみ変換を適用します。失敗時はそのまま空の Lax が伝播します
Div[100, count]()
  .map(_ x = x * taxRate)
  .map(_ x = Round[x]().unmold())
  ]=> taxAmount
```

Lax の詳細は [リスト操作](06_lists.md) および [モールディング型リファレンス](../reference/mold_types.md) を参照してください。

---

## Error 基底型

カスタムエラーはすべて `Error` 基底型を継承して定義します。`throw()` メソッドは Error 型を継承した型のインスタンスのみが持ちます。

```taida
// Error 基底型（組み込み）
Error = @(
  type: Str
  message: Str
)

// カスタムエラーの定義
Error => ValidationError = @(
  field: Str
  code: Int
)

Error => ApiError = @(
  status: Int
  endpoint: Str
)
```

### throw は Error 継承型のみ

```taida
// OK: Error を継承しています
ValidationError(type <= "ValidationError", message <= "Invalid", field <= "email", code <= 400).throw()

// NG: 通常のぶちパックは throw できません
@(name <= "Asuka").throw()  // コンパイルエラー
```

---

## throw と |== エラー天井

### 基本構文

`|==` は、その下で発生する `throw` をキャッチするエラー天井です。式ではなく**スコープ宣言**です — `|==` の下にある全てのコードが保護領域となり、throw が発生するとハンドラが実行されます。

```taida
|== error: ErrorType =
  | 条件1 |> 処理1
  | 条件2 |> 処理2
  | _ |> デフォルト処理
=> :ReturnType

// ここから下が保護領域（throw が発生する可能性のある処理）
```

ハンドラの戻り値型（`=> :ReturnType`）は、保護領域の戻り値型と一致しなければなりません。型が合わない場合はコンパイルエラーになります。

> **C13-1 の末尾バインド短縮形**: `|==` ハンドラ本体の末尾が `name <= expr` / `expr => name` / `expr ]=> name` / `name <=[ expr` であるとき、束縛された値がそのままハンドラの結果になります。例:
>
> ```taida
> |== error: Error =
>   fallback <= "default"   // 末尾バインド — ハンドラ結果は "default"
> => :Str
> ```
>
> 関数本体や `| |>` アーム本体と同じ規則が適用されます。詳細は `docs/guide/07_control_flow.md` を参照してください。

### 基本的な使用例

```taida
Error => ValidationError = @(field: Str)

processInput text: Str =
  |== error: Error =
    | error.type == "ValidationError" |> "Invalid: empty"
    | _ |> "Unknown error: " + error.message
  => :Str

  | text == "" |> ValidationError(type <= "ValidationError", message <= "Empty input", field <= "text").throw()
  | _ |> "Processed: " + text
=> :Str
```

### エラーの伝播

`throw` は最も近い `|==` に到達するまで、呼び出しスタックを遡って伝播します。

- 同一スコープに `|==` がある → そのハンドラでキャッチされます
- 同一スコープに `|==` がない → 呼び出し元のスコープに伝播します
- HOF（Map、Filter 等）のコールバック内で `throw` が発生した場合も同様に伝播します
- どこにも `|==` がなければ → ゴリラ天井に到達し、プログラムは終了します

```taida
Error => NegativeValueError = @(value: Int)

innerFunction value: Int =
  | value < 0 |> NegativeValueError(type <= "NegativeValueError", message <= "Must be positive", value <= value).throw()
  | _ |> value * 2
=> :Int

outerFunction x: Int =
  |== error: Error =
    | error.type == "NegativeValueError" |> 0
    | _ |> -1
  => :Int

  result <= innerFunction(x)  // ここで throw が発生する可能性があります
  result + 10
=> :Int

result <= outerFunction(-5)  // 0 (エラーがキャッチされました)
result <= outerFunction(5)   // 20 (正常処理)
```

### 複数エラーの処理

```taida
Error => NegativeError = @(value: Int)
Error => TooLargeError = @(value: Int, limit: Int)

processValue value: Int =
  |== error: Error =
    | error.type == "NegativeError" |> @(success <= false, result <= 0, error <= "Negative")
    | error.type == "TooLargeError" |> @(success <= false, result <= 0, error <= "Too large")
    | _ |> @(success <= false, result <= 0, error <= "Unknown")
  => :@(success: Bool, result: Int, error: Str)

  | value < 0 |> NegativeError(type <= "NegativeError", message <= "Negative", value <= value).throw()
  | value > 1000 |> TooLargeError(type <= "TooLargeError", message <= "Over limit", value <= value, limit <= 1000).throw()
  | _ |> @(success <= true, result <= value * 2, error <= "")
=> :@(success: Bool, result: Int, error: Str)
```

---

## ゴリラ天井（Gorilla Ceiling）

明示的な `|==` がない場合、プログラムのトップレベルには暗黙の**ゴリラ天井**が存在します。

```
あなたのプログラム
|-- 関数A
|   +-- 関数B（ここでエラーが投げられます）
|       ! |== がありません
|-- ゴリラ天井
|   +-- 誰もキャッチしなかったので、ゴリラがプログラムを停止させます
+-- プログラム終了
```

ゴリラ天井は最後の番人です。すべてのエラーは最終的にゴリラ天井に到達し、プログラムは終了します。エラーが野放しになることを許しません。

```taida
// この関数にはエラー天井がありません
riskyFunction x: Int =
  | x < 0 |> Error(type <= "RuntimeError", message <= "Negative value").throw()
  | _ |> x * 2
=> :Int

// throw が発生すると、ゴリラ天井がキャッチしてプログラムは終了します
result <= riskyFunction(-1)  // ゴリラ天井に到達 => プログラム終了
```

### なぜゴリラ天井が必要なのか

他の言語ではエラーが黙殺されることがあります。Python の `None` が伝播して、全然関係ないところで `AttributeError` が出ます。JavaScript の未処理 Promise は静かに失敗します。

Taida ではカスタムエラーの throw はゴリラ天井が確実にキャッチし、プログラムを停止させます。一方、ゼロ除算や範囲外アクセスなどの「よくある失敗」は Lax で安全に処理され、プログラムは止まりません。

```taida
// 致命的なエラー: ゴリラ天井が停止させます
Error => FatalError = @(reason: Str)
FatalError(type <= "FatalError", message <= "Critical failure", reason <= "disk full").throw()
// ゴリラ天井に到達 => プログラム終了

// よくある失敗: Lax で安全に処理されます
Div[10, 0]() ]=> result  // 0 (プログラムは止まりません)
```

エラーを握り潰したい場合は、明示的に `|==` を書く必要があります。何もしなければ、ゴリラがプログラムを停止させます。

---

## Result[T, P] -- 述語付き操作モールド

Result は「成功条件を述語で定義する」モールド型です。`]=>` でアンモールディングすると述語 P が評価され、真なら値 T を返し、偽なら throw が発動します。

```taida
Mold[T] => Result[T, P <= :T => :Bool] = @(throw: Error)
// P: :T => :Bool（成功条件を定義する述語）
```

### 基本的な使用

```taida
// _ = true は常に真を返す無名関数（常に成功）
ok <= Result[42, _ = true]()
ok ]=> value  // 42

// _ = false は常に偽を返す無名関数（常に失敗）
Error => MyError = @(reason: Str)
err <= Result[0, _ = false](throw <= MyError(type <= "MyError", message <= "fail", reason <= "invalid"))
err ]=> value  // throw が発動（エラー天井で捕捉可能）
```

### => と ]=> の違い

`=>` は Result をそのまま代入します。`]=>` は述語を評価して値を取り出します。

```taida
Result[x, pred](throw <= err) => r   // r: Result[T, P]（述語は未評価）
Result[x, pred](throw <= err) ]=> r  // r: T（述語を評価 → 真なら値、偽なら throw）
```

### 述語によるバリデーション

述語にラムダを渡すことで、値のバリデーションを表現できます。

```taida
Error => ValidationError = @(field: Str)

// 年齢バリデーション: 18歳以上なら成功
age <= 25
Result[age, _ x = x >= 18]() ]=> validAge  // 25 >= 18 → true → validAge = 25

age <= 15
Result[age, _ x = x >= 18](throw <= ValidationError(type <= "ValidationError", message <= "Must be 18+", field <= "age")) ]=> validAge
// 15 >= 18 → false → throw 発動
```

### 関数の戻り値として

推論可能である場合、戻り値の型注釈では `:Result[T, _]` と書くことができます。`_` は述語の型を推論させるプレースホルダです。

```taida
Error => ValidationError = @(field: Str)

validateAge age: Int =
  Result[age, _ x = x >= 18](throw <= ValidationError(type <= "ValidationError", message <= "Must be 18+", field <= "age"))
=> :Result[Int, _]

// 呼び出し側
validateAge(25) ]=> validAge   // 25（成功）
validateAge(15) ]=> validAge   // throw 発動（エラー天井で捕捉可能）

// Result として受け取る（述語未評価）
validateAge(15) => result      // result: Result[Int, _]（まだ throw しない）
result ]=> validAge            // ここで述語が評価されて throw
```

### map / flatMap によるチェーン

Result はモナディック操作をサポートしています。

```taida
result <= Result[42, _ = true]()

// 成功時のみ変換します
doubled <= result.map(_ x = x * 2)

// 失敗時はそのまま伝播します
failed <= Result[0, _ = false](throw <= Error(type <= "Error", message <= "oops"))
failed.map(_ x = x * 2)  // throw がセットされたまま伝播
```

### getOrDefault / getOrThrow

```taida
result <= Result[42, _ = true]()

result.getOrDefault(0)   // 42
result.getOrThrow()      // 42

failed <= Result[0, _ = false](throw <= Error(type <= "Error", message <= "oops"))

failed.getOrDefault(0)   // 0
failed.getOrThrow()      // throw が発動します
```

---

## Gorillax -- 覚悟のエラーモデル

外部パッケージ（npm 等）の Molten（溶鉄）操作は安全が保証されません。Cage モールドで Molten への操作を実行し、結果を Gorillax で受け取ります。Cage は Molten 専用です。

### 基本: Cage → Gorillax → unmold

```taida
>>> npm:lodash => @(lodash)  // lodash: Molten

items: @[Int] <= @[1, 2, 3]
Cage[lodash, _ lo = lo.sum(items)] => rilax  // rilax: Gorillax[Int]
rilax ]=> total  // 成功 → total = 6, 失敗 → ゴリラ（プログラム終了）
```

Gorillax の unmold 失敗はゴリラと同じ -- `|==` ではキャッチできません。

### relax: キャッチ可能にする

`.relax()` を呼ぶと `RelaxedGorillax[T]` に変換されます。この型は unmold 失敗時に `RelaxedGorillaEscaped` エラーを throw します。`|==` でキャッチ可能です。

```taida
rilax.relax() => relaxed  // relaxed: RelaxedGorillax[Int]

|== error: RelaxedGorillaEscaped =
  | _ |> 0  // フォールバック値
=> :Int

relaxed ]=> total  // 失敗時: RelaxedGorillaEscaped が throw → |== でキャッチ
```

### エラーモデルの全体像

```
Lax 層:        失敗 → デフォルト値（プログラム続行）
throw 層:      失敗 → |== でキャッチ、またはゴリラ天井
Gorillax 層:   失敗 → ゴリラ（プログラム即終了）
  └ .relax():  失敗 → RelaxedGorillaEscaped（|== でキャッチ可能）
```

---

## パターン: エラーハンドリングのベストプラクティス

### 1. よくある失敗は Lax に任せる

ゼロ除算や範囲外アクセスのために throw/|== を書く必要はありません。Lax がすべて安全に処理します。

```taida
// 良い例: Lax に任せます
calculateAverage scores: @[Int] =
  Sum[scores]() ]=> total
  Div[total, scores.length()]() ]=> avg
  avg
=> :Int

// 不要: ゼロ除算のためにエラー処理を書く必要はありません
// | scores.isEmpty() |> Error(...).throw()  // これは過剰です
```

### 2. ビジネスロジックのエラーは throw で

ビジネスルールの違反など、処理を続行すべきでない場合は throw を使います。

```taida
Error => AuthError = @(userId: Str)
Error => PermissionError = @(action: Str)

deleteUser userId: Str requesterId: Str =
  |== error: Error =
    | error.type == "AuthError" |> @(success <= false, message <= "Authentication failed")
    | error.type == "PermissionError" |> @(success <= false, message <= "Permission denied")
    | _ |> @(success <= false, message <= "Unknown error")
  => :@(success: Bool, message: Str)

  | requesterId == "" |> AuthError(type <= "AuthError", message <= "Not authenticated", userId <= requesterId).throw()
  | requesterId != "admin" |> PermissionError(type <= "PermissionError", message <= "Admin only", action <= "delete").throw()
  | _ |>
    performDelete(userId)
    @(success <= true, message <= "Deleted")
=> :@(success: Bool, message: Str)
```

### 3. ガード節 + ゴリラで致命的な前提条件を守る

回復不能な前提条件の違反には `><` を使います。

```taida
initializeSystem config =
  | config.dbUrl == "" |> ><             // DB接続先なしは致命的です
  | config.port < 0 |> ><               // 不正なポートも致命的です
  | config.port > 65535 |> ><           // ポート範囲外も致命的です
  | _ |>
    db <= connectDatabase(config.dbUrl)
    startServer(config.port, db)
=> :Server
```

### 4. バリデーションチェーン

```taida
Error => ValidationError = @(field: Str)

validateAndProcess input =
  |== error: Error =
    @(success <= false, errors <= @[error.message])
  => :@(success: Bool, errors: @[Str])

  | input.name == "" |> ValidationError(type <= "ValidationError", message <= "Name required", field <= "name").throw()
  | input.email == "" |> ValidationError(type <= "ValidationError", message <= "Email required", field <= "email").throw()
  | input.age < 18 |> ValidationError(type <= "ValidationError", message <= "Must be 18+", field <= "age").throw()
  | _ |>
    result <= process(input)
    @(success <= true, errors <= @[])
=> :@(success: Bool, errors: @[Str])
```

---

## まとめ

| 概念 | 構文 | 説明 |
|------|------|------|
| **Lax** | `Div[x, y]()`, `get()` 等 | よくある失敗を安全に処理します。デフォルト値にフォールバックします |
| **Error 定義** | `Error => MyError = @(...)` | カスタムエラー型を定義します |
| **throw** | `.throw()` | Error 継承型のインスタンスでエラーを発生させます |
| **エラー天井** | `\|==` | throw されたエラーをキャッチして処理します |
| **ゴリラ天井** | 暗黙 | `\|==` がない場合の最後の番人です。ゴリラがプログラムを停止させます |
| **Result** | `Result[T, P]` | 述語 P で成功/失敗を判定する操作モールド型です |
| **ゴリラ** | `><` | 即時終了リテラルです。条件分岐と組み合わせて使います |
| **Gorillax** | `Gorillax[T]` | 覚悟のモールド型。unmold 失敗でゴリラ（プログラム終了） |
| **Cage** | `Cage[Molten, F]` | Molten 専用の操作檻。F(Molten) を実行し Gorillax[U] を返します |
| **RelaxedGorillax** | `.relax()` | Gorillax を throw 可能に変換。`\|==` でキャッチ可能になります |

### 使い分けの指針

| 状況 | 使うべきもの |
|------|------------|
| ゼロ除算、範囲外アクセス | Lax（自動的に安全） |
| 型変換の失敗 | Lax（`Int[x]()` 等が Lax を返す） |
| ビジネスルールの違反 | `throw` + `\|==` |
| 回復不能な前提条件の違反 | `><` ゴリラリテラル |
| 関数の戻り値としてのエラー表現 | `Result[T, P]`（`:Result[T, _]` で型推論） |
| 外部パッケージの Molten 操作 | `Cage` → `Gorillax`（失敗は致命的） |
| 外部操作の失敗をキャッチしたい | `.relax()` → `RelaxedGorillax` + `\|==` |
| エラーを握り潰さず確実に止めたい | ゴリラ天井（何もしなければ自動） |
