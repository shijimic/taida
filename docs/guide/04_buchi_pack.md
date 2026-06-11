# ぶちパック構文

> **PHILOSOPHY.md — II.** だいじなものはふくろにしまっておきましょう

> 推奨読書順序: 本章 (`04_buchi_pack.md` — 値リテラル) → [クラスライク型定義](04_class_like.md) (`04_class_like.md` — 型定義) の順。番号 04 を共有する 2 章は、値と型を意図的に分離しており、値リテラル `@(...)` / `@[...]` を先に把握してから型定義側に進むと、`@(...)` が値と型でどう使い分けられるかが理解しやすくなります。

> 型定義 (`Pilot = @(...)`) と継承 (`Pilot => NervStaff = @(...)`) は **クラスライク型の単一概念** に統合されており、[クラスライク型定義](04_class_like.md) で説明します。本章では値リテラル `@(...)` / `@[...]` を中心に扱います。

---

## ぶちパックとは

名前付きフィールドの集合を表現する構造化データ記法です。関連するデータをひとまとめにして名前をつけ、安全に持ち運びます。ふくろにしまう、ということです。

```taida
pilot <= @(name <= "Asuka", age <= 14, active <= true)
```

ぶちパックはあくまで **値リテラル** の構文です。型を定義する場合は [クラスライク型定義](04_class_like.md) を参照してください。

---

## 値リテラル `@(...)`

### 値の作成

```taida
pack <= @(name <= "Asuka", age <= 14, active <= true)
```

### フィールドアクセス

```taida
pilot <= @(name <= "Shinji", age <= 14, call_sign <= "Ikari")

pilotName <= pilot.name        // "Shinji"
pilotAge <= pilot.age          // 14
pilot_call <= pilot.call_sign  // "Ikari"
```

### 存在しないフィールド

存在しないフィールドにアクセスするとコンパイルエラーになります:

```taida fragment
pilot <= @(name <= "Rei")
sync_rate <= pilot.sync_rate  // コンパイルエラー: フィールド 'sync_rate' は存在しません
```

ただし、クラスライク型として宣言されたフィールドはデフォルト値を持つため、宣言されたフィールドを省略してインスタンス化することは可能です。詳細は [クラスライク型定義 / デフォルト値](04_class_like.md#デフォルト値) を参照してください。

---

## 即時評価

ぶちパックのフィールドは代入時に即座に評価されます。

```taida
nowMs =
  1234567890
=> :Int

heavyCalc n: Int =
  n * n + 1
=> :Int

data <= @(
  timestamp <= nowMs(),             // 代入時に即座に評価されます
  computed <= heavyCalc(6)          // 代入時に即座に評価されます
)

// この時点で全フィールドは評価済みです
ts <= data.timestamp  // キャッシュされた値を取得します
```

これはスコープベース管理と組み合わさって安全性を保証します。参照先が解放されても、値はコピーされているため問題ありません。

---

## スコープベース管理

全てのぶちパック変数は、定義されたスコープで自動管理されます:

```taida
config <= @(host <= "localhost", port <= 8080)  // モジュールスコープ

processData items: @[Int] =
  temp <= @(processed <= items.length(), timestamp <= nowMs())
  result <= @(count <= temp.processed, time <= temp.timestamp)
  result  // 戻り値として返されます
=> :@(count: Int, time: Int)
// temp は関数終了時に自動解放されます
```

関数引数 `items` には型注釈 (`: @[Int]`) を明示しています。引数型は推論可能な文脈なら省略できますが、ガイドラインとしては明示する方を推奨します。詳細は [型システム / 関数の型指定](01_types.md#関数の型指定) を参照してください。

---

## 型定義との関係

ぶちパックの **値リテラル** (`@(name <= "Asuka")`) と、それを受け取る **型定義** (`Pilot = @(name: Str)`) は別の概念です。型定義は [クラスライク型定義](04_class_like.md) の単一構文で書きます。本章は値リテラル中心です。

簡単に対応関係を示します:

| 用途 | 構文 | 章 |
|------|------|-----|
| 値リテラル (構造化データを作る) | `@(name <= "Asuka")` | 本章 |
| クラスライク型定義 | `Pilot = @(name: Str)` | [04_class_like.md](04_class_like.md) |
| インスタンス化 | `Pilot(name <= "Asuka")` | [04_class_like.md](04_class_like.md) |
| 継承 | `Pilot => NervStaff = @(...)` | [04_class_like.md](04_class_like.md) |
| 構造的部分型 | `HasName = @(name: Str)` 経由 | [04_class_like.md](04_class_like.md) |

---

## ネスト構造

ぶちパックは直接ネストできます:

```taida
pilot <= @(
  name <= "Rei",
  contact <= @(
    email <= "rei@nerv.jp",
    phone <= "NERV-001"
  )
)

email <= pilot.contact.email  // "rei@nerv.jp"
```

クラスライク型でも同様にネストできます。詳細は [クラスライク型定義 / ネスト構造](04_class_like.md#ネスト構造) を参照してください。

---

## 関数を含むぶちパック

ぶちパックのフィールドには lambda を束縛できます。フィールドは常に `name <= 値` の名前付き形なので、関数フィールドも `name <= _ 引数... = 本体` の lambda 束縛で書きます (`add x: Int ... = ...` のような関数定義構文をフィールド位置に直接書くことはできません — `[E1521]`):

```taida
mathUtils <= @(
  add <= _ x: Int y: Int = x + y,
  subtract <= _ x: Int y: Int = x - y,
  PI <= 3.14159
)

result <= mathUtils.add(3, 5)  // 8
pi <= mathUtils.PI             // 3.14159
```

---

## 関数への名前付き引数

```taida fragment
connect host: Str port: Int timeout: Int =
  connection
=> :Connection

options <= @(host <= "localhost", port <= 8080, timeout <= 5000)
conn <= connect(options)  // パラメータが自動展開されます
```

---

## モールド型との統合

ぶちパック内にモールド型を格納できます。

```taida
pilot_data <= @(
  name <= "Asuka",
  sync_rate <= Div[780, 10]()
)

// >=> でアンモールド
pilot_data.sync_rate >=> rate_value  // 78
```

モールド型の詳細は [モールド](05_mold.md) を参照してください。

---

## リストリテラル `@[...]`

リストは `@[...]` で表現します:

```taida
numbers <= @[1, 2, 3, 4, 5]
names <= @["Asuka", "Shinji", "Rei"]
empty: @[Int] <= @[]

// 型付きリスト
pilots: @[Pilot] <= @[
  Pilot(name <= "Asuka", age <= 14),
  Pilot(name <= "Shinji", age <= 14)
]
```

### `@[]` (空リスト) と `@()` (空ぶちパック) は別カテゴリ

| 形 | 意味 | 扱い |
|----|------|------|
| `@[]` | 空のリスト (要素 0) | OK。明確な意味を持つ値 |
| `@()` 値リテラル | フィールドなしのぶちパック値 | 言語仕様としては書く場面が存在しない (ぶちパックは必ず名前付きフィールド集合) |
| `@()` 型注釈 / 戻り型 / 引数型 / class-like body | 「情報なしを意味する型」 | `[E1520]` で reject。null と同レベルの抜け道として禁止 |

「ぶちパックのフィールドが全部なくなるまで削る」操作は Taida 言語には存在しません。フィールド集合を変える関数は、戻り型を別の具体ぶちパック型として定義するのが正解です。

> **PHILOSOPHY.md — II.** だいじなものはふくろにしまっておきましょう

の系「ふくろの中身が変わったら、別のふくろにしまいなおす」を参照してください。

```taida fragment
// OK: 縮小型を明示
removePrice item: @(name: Str, price: Int) = ... => :@(name: Str)

// NG: 戻り型として `:@()` は書けない (`[E1520]`)
// removeName item: @(name: Str) = ... => :@()
```

---

## JSON からの変換

外部の JSON データをぶちパックに変換するには、`JSON[raw, Schema]()` モールドを使います。詳細は [JSON 溶鉄](03_json.md) を参照してください。

```taida
Pilot = @(name: Str, age: Int)

raw <= '{"name": "Ritsuko", "age": 30}'
JSON[raw, Pilot]() >=> pilot
// pilot: @(name <= "Ritsuko", age <= 30)
```

---

## まとめ

| 概念 | 構文 | 章 |
|------|------|-----|
| 値の作成 | `@(field <= value, ...)` | 本章 |
| フィールドアクセス | `pack.fieldName` | 本章 |
| ネスト | `@(inner <= @(field <= value))` | 本章 |
| リストリテラル | `@[elem1, elem2, ...]` | 本章 |
| JSON 変換 | `JSON[raw, TypeName]() >=> val` | [03_json.md](03_json.md) |
| クラスライク型定義 | `Name[?type-args] [=> Parent] = @(...)` | [04_class_like.md](04_class_like.md) |
| モールド | `Upper[str]()` 等 | [05_mold.md](05_mold.md) |

型の継承記述は [クラスライク型定義 / 継承](04_class_like.md#継承) を参照してください。
