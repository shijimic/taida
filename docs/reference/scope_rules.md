# スコープ規則

## 概要

Taida Langには2種類のスコープがあります。

| スコープ | 説明 |
|----------|------|
| モジュールスコープ | ファイルのトップレベルで定義されたシンボル |
| 関数スコープ | 関数内で定義されたシンボル |

---

## モジュールスコープ

ファイルのトップレベルで定義された型、関数、変数はモジュールスコープに属します。

```taida
// モジュールスコープの定義
Pilot = @(name: Str, age: Int)  // 型定義

MAX_SYNC_RATE <= 100  // 定数

getPilot id: Int =
  // 関数定義
=> :Pilot

// モジュール内のどこからでもアクセス可能
pilot <= Pilot(name <= "Misato", age <= MAX_SYNC_RATE)
```

### モジュールスコープの特徴

- ファイル内のどこからでもアクセス可能
- `<<<` でエクスポートすると他のモジュールからもアクセス可能
- インポートしたシンボルもモジュールスコープで利用可能

```taida
>>> ./utils.td => @(helper)  // helperがモジュールスコープに追加される

result <= helper(data)  // どこからでも使用可能

processData input: Str =
  helper(input)  // 関数内でも使用可能
=> :Str
```

---

## 関数スコープ

関数内で定義された変数、内部関数はその関数内でのみ有効です。

```taida
processPilot pilot: Pilot =
  // 引数 pilot は関数スコープ
  name <= pilot.name  // name は関数スコープ

  // 内部関数も関数スコープ
  formatName =
    name + " さん"
  => :Str

  formatted <= formatName()
  formatted
=> :Str

// NG: 関数外からはアクセス不可
// name  // コンパイルエラー
// formatName  // コンパイルエラー
```

### 関数スコープのネスト

内部関数は外側のスコープにアクセスできます（レキシカルスコープ）。

```taida
outer x: Int =
  y <= 10  // outer の関数スコープ

  inner z: Int =
    // x, y, z すべてにアクセス可能
    x + y + z
  => :Int

  inner(5)
=> :Int

result <= outer(3)  // 18 (3 + 10 + 5)
```

---

## 条件分岐内のスコープ

条件分岐 `|` `|>` 内で定義された変数は、その分岐内でのみ有効です。

```taida
processValue value: Int =
  // 複数行の多アーム条件は丸括弧で包みます（C20 / E0303）。
  result <= (
    | value > 100 |>
      message <= "large"  // この分岐内でのみ有効
      message
    | value > 50 |>
      message <= "medium"  // 別の分岐、同名でも別変数
      message
    | _ |>
      "small"
  )

  // message はここからアクセス不可
  result
=> :Str
```

### 分岐外で値を使いたい場合

分岐の戻り値として受け取ります。

```taida
processValue value: Int =
  // 分岐全体の結果を result に代入
  result <= (
    | value > 100 |>
      message <= "large: " + value
      @(category <= "large", display <= message)
    | _ |>
      @(category <= "small", display <= "small value")
  )

  // result を通じてアクセス
  result.category  // OK
  result.display   // OK
=> :@(category: Str, display: Str)
```

---

## エラー天井内のスコープ

エラー天井 `|==` 内で定義された変数は、そのエラー天井内でのみ有効です。

```taida
processData data: Data =
  |== error: Error =
    // error はこのブロック内でのみ有効
    message <= "Error: " + error.message
    @(success <= false, error_message <= message)
  => :@(success: Bool, error_message: Str)

  // error, message はここからアクセス不可
  processed <= doProcess(data)
  @(success <= true, error_message <= "")
=> :@(success: Bool, error_message: Str)
```

---

## シャドウイング

同一スコープ内での同名変数の再定義（シャドウイング）は禁止されています。

```taida
// NG: 同一スコープでの再定義
x <= 10
x <= 20  // コンパイルエラー: 'x' は既に定義されています

// NG: 関数内での再定義
processData data =
  temp <= data.value
  temp <= temp * 2  // コンパイルエラー
=> :Int
```

### 異なるスコープでは許可

```taida
x <= 10  // モジュールスコープ

process =
  x <= 20  // 関数スコープ、これは許可される（ただし外側のxを隠す）
  x
=> :Int

// 外側の x は変更されていない
result <= x  // 10
```

> **注意**: 異なるスコープでのシャドウイングは許可されますが、混乱を避けるため推奨されません。

---

## 再代入の禁止

Taida Langではすべての変数はイミュータブルです。一度定義した変数に再代入することはできません。

```taida
// NG: 再代入
counter <= 0
counter <= counter + 1  // コンパイルエラー

// OK: 新しい変数を定義
counter_initial <= 0
counter_updated <= counter_initial + 1
```

### イミュータブルな更新パターン

```taida
// リストの「更新」
original <= @[1, 2, 3]
// 新しいリストを生成
Map[original, _ x = x * 2]() ]=> updated
// original: @[1, 2, 3] (変更されない)
// updated: @[2, 4, 6]

// ぶちパックの「更新」
pilot <= @(name <= "Misato", age <= 14)
// 新しいぶちパックを生成
updated_pilot <= @(name <= pilot.name, age <= pilot.age + 1)
```

---

## スコープのまとめ

```taida
// === モジュールスコープ ===
Config = @(timeout: Int)  // 型定義
DEFAULT_TIMEOUT <= 5000   // 定数
config <= Config(timeout <= DEFAULT_TIMEOUT)  // 変数

processRequest request: Request =
  // === 関数スコープ ===
  // request は引数（関数スコープ）
  timeout <= config.timeout  // 外側のconfigにアクセス可能

  helper =
    // === ネストした関数スコープ ===
    // timeout, request にアクセス可能
    request.data
  => :Data

  result <= (
    | request.valid |>
      // === 分岐スコープ ===
      data <= helper()
      processData(data)
    | _ |>
      // === 別の分岐スコープ ===
      @(error <= "invalid")
  )

  result
=> :Result

// モジュールスコープからは Config, DEFAULT_TIMEOUT, config, processRequest にアクセス可能
// timeout, helper, result などにはアクセス不可
```
