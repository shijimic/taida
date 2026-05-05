# ぶちパック構文

> **PHILOSOPHY.md -- II.** だいじなものはふくろにしまっておきましょう

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

```taida
pilot <= @(name <= "Rei")
sync_rate <= pilot.sync_rate  // コンパイルエラー: フィールド 'sync_rate' は存在しません
```

ただし、クラスライク型として宣言されたフィールドはデフォルト値を持つため、宣言されたフィールドを省略してインスタンス化することは可能です。詳細は [クラスライク型定義 / デフォルト値](04_class_like.md#デフォルト値) を参照してください。

---

## 即時評価

ぶちパックのフィールドは代入時に即座に評価されます。

```taida
data <= @(
  timestamp <= getCurrentTime(),    // 代入時に即座に評価されます
  computed <= heavyCalculation()    // 代入時に即座に評価されます
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

processData items =
  temp <= @(processed <= items.length(), timestamp <= now())
  result <= @(count <= temp.processed, time <= temp.timestamp)
  result  // 戻り値として返されます
=> :@(count: Int, time: Str)
// temp は関数終了時に自動解放されます
```

---

## 型定義との関係

ぶちパックの **値リテラル** (`@(name <= "Asuka")`) と、それを受け取る **型定義** (`Pilot = @(name: Str)`) は別の概念です。E30 では型定義は [クラスライク型定義](04_class_like.md) の単一構文に統一されているため、本章では深入りしません。

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

ぶちパック内に関数を定義できます:

```taida
mathUtils <= @(
  add x: Int y: Int = x + y => :Int,
  subtract x: Int y: Int = x - y => :Int,
  PI <= 3.14159
)

result <= mathUtils.add(3, 5)  // 8
pi <= mathUtils.PI             // 3.14159
```

---

## 関数への名前付き引数

```taida
connect host: Str port: Int timeout: Int =
  // 接続処理
=> :Connection

options <= @(host <= "localhost", port <= 8080, timeout <= 5000)
conn <= connect(options)  // パラメータが自動展開されます
```

---

## モールディング型との統合

ぶちパック内にモールディング型を格納できます:

```taida
pilot_data <= @(
  name <= "Asuka",
  sync_rate <= Div[780, 10]()
)

// ]=> でアンモールディング
pilot_data.sync_rate ]=> rate_value  // 78
```

モールディング型の詳細は [操作モールド](05_molding.md) を参照してください。

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

---

## JSON からの変換

外部の JSON データをぶちパックに変換するには、`JSON[raw, Schema]()` モールドを使います。詳細は [JSON 溶鉄](03_json.md) を参照してください。

```taida
Pilot = @(name: Str, age: Int)

raw <= '{"name": "Ritsuko", "age": 30}'
JSON[raw, Pilot]() ]=> pilot
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
| JSON 変換 | `JSON[raw, TypeName]() ]=> val` | [03_json.md](03_json.md) |
| クラスライク型定義 | `Name[?type-args] [=> Parent] = @(...)` | [04_class_like.md](04_class_like.md) |
| 操作モールド | `Upper[str]()` 等 | [05_molding.md](05_molding.md) |

旧 D 世代までの「型の継承（InheritanceDef）」記述は [クラスライク型定義 / 継承](04_class_like.md#継承) に統合されました。
