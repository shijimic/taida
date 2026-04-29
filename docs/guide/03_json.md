# JSON 溶鉄

> **PHILOSOPHY.md -- III.** カタめたいなら、鋳型を作りましょう

---

## JSON は溶鉄です

Taida の内部は型安全な世界です。全ての変数に型があり、デフォルト値があり、null はありません。

しかし外部の世界 -- API レスポンス、設定ファイル、ユーザー入力 -- はそうとは限りません。JSON には `null` が含まれるかもしれませんし、フィールドが欠けているかもしれません。型が間違っている可能性もあります。

Taida は JSON を**溶鉄**（molten iron）として扱います。`@e.X` 以降のモデルでは
`Molten => JSON` の分岐です。溶鉄は熱く、形がなく、そのままでは触れません。型安全な世界に持ち込むには、必ず**鋳型**（スキーマ）を通す必要があります。

```
溶鉄（JSON）         鋳型（スキーマ）         製品（型付きぶちパック）
                    +-----------+
  raw string        |           |
  API response  --> | JSON[x,T] | -->  型安全な Taida 値
  config file       |           |
                    +-----------+
```

- **溶鉄** = 生の JSON データ。形がありません。触れません。そのままでは何もできません。
- **鋳型** = ぶちパックの型定義（TypeDef）。JSON の形を決めます。
- **鋳造** = `JSON[raw, Schema]()` で溶鉄を鋳型に流し込みます。
- **製品** = 型付きぶちパック。Taida の型安全な世界の住人です。

### JSON は不透明なプリミティブ型です

JSON 型はメソッドを一切持ちません。JSON はただの不透明な塊であり、中身を覗くことも、直接操作することもできません。`Molten[Str]` のような型引数つきの Molten でもなく、Cage で扱う JS 分岐でもありません。

```taida
// これらは全てエラーになります
data.at("name")       // エラー: JSON has no methods
data.toStr()           // エラー: JSON has no methods
data.keys()            // エラー: JSON has no methods
data ]=> x             // エラー: JSON direct unmold is not allowed
```

JSON を使うには、スキーマを指定して `JSON[raw, Schema]()` で型変換するしかありません。`Cage` は JSON 分岐を受け取らず、JS / npm 連携の `Molten => JS` 分岐だけを扱います。

---

## JSON[raw, Schema]() の基本

`JSON[raw, Schema]()` モールドが、溶鉄を型安全な値に鋳造する唯一の方法です。

### 引数の順序: `[何を, 何で]` 原則

```taida
JSON[raw, User]()
      |     |
     何を  何で（どの型に鋳造するか）
```

他のモールドと同じく、対象（何を）が先、ツール/パラメータ（何で）が後です。`Filter[list, fn]()`, `Split[str, delim]()` と一貫しています。

### 基本の使い方

```taida
// 1. スキーマを定義します
Pilot = @(name: Str, age: Int, active: Bool)

// 2. JSON 文字列を用意します
raw <= '{"name": "Asuka", "age": 14, "active": true}'

// 3. JSON モールドでスキーマを通して鋳造します
JSON[raw, Pilot]() ]=> pilot
// pilot: @(name <= "Asuka", age <= 14, active <= true)
```

### 2 引数は必須です

`JSON` モールドは常に 2 引数（生データ + スキーマ）を要求します。1 引数はコンパイルエラーです。

```taida
JSON[raw, User]()    // OK: スキーマあり
JSON[raw]()          // エラー: JSON requires a schema type argument
```

---

## スキーマの書き方

スキーマは Taida の TypeDef（ぶちパックの型定義）をそのまま使います。特別な構文は必要ありません。

### 基本スキーマ

```taida
Pilot = @(
  name: Str
  age: Int
  active: Bool
)

raw <= '{"name": "Rei", "age": 14, "active": true}'
JSON[raw, Pilot]() ]=> pilot
```

### ネストしたスキーマ

```taida
Address = @(
  city: Str
  zip: Str
)

Pilot = @(
  name: Str
  age: Int
  address: Address
)

raw <= '{"name": "Asuka", "age": 14, "address": {"city": "Tokyo-3", "zip": "999-0003"}}'
JSON[raw, Pilot]() ]=> pilot
pilot.address.city    // "Tokyo-3"
```

### リスト型のスキーマ

配列の JSON は `@[TypeDef]` をスキーマとして渡します。

```taida
Pilot = @(name: Str, sync_rate: Int)

raw <= '[{"name": "Asuka", "sync_rate": 95}, {"name": "Shinji", "sync_rate": 41}]'
JSON[raw, @[Pilot]]() ]=> pilots
// pilots: @[@(name <= "Asuka", sync_rate <= 95), @(name <= "Shinji", sync_rate <= 41)]
```

### プリミティブ型への直接変換

```taida
JSON['"42"', Int]() ]=> num        // 42
JSON['[1, 2, 3]', @[Int]]() ]=> nums  // @[1, 2, 3]
```

### Enum 型フィールドの検査

Enum 型はスキーマの一級市民です。`JSON` モールドは JSON 側の文字列が variant 集合に含まれることを**検査します**。

- 一致したとき → そのバリアントの ordinal（`Value::Int`）を返します
- 一致しなかった / キーが無かった / `null` だった → **`Lax[Enum]`** を返します（silent coercion は行いません）

```taida
Enum => Status = :Active :Inactive :Pending

User = @(name: Str, status: Status)

// variant 一致 → ordinal が入ります
raw1 <= '{"name": "Alice", "status": "Active"}'
JSON[raw1, User]() ]=> u1
u1.status                  // 0（Active の ordinal）

// 不正な文字列 → Lax[Enum] で境界が明示されます
raw2 <= '{"name": "Dave", "status": "Bogus"}'
JSON[raw2, User]() ]=> u2
u2.status.hasValue         // false
u2.status.getOrDefault(Status:Pending())  // 2（呼び出し側がフォールバックを決めます）

// キー欠落も Lax[Enum] です
raw3 <= '{"name": "Eve"}'
JSON[raw3, User]() ]=> u3
u3.status.hasValue         // false
```

この挙動は PHILOSOPHY の「**暗黙の型変換なし**」を境界で再現するものです。JSON に書かれた任意の Str が Enum 値として黙って通ることはなく、利用側は `hasValue` / `| .hasValue |> ... | _ |> ...` / `getOrDefault(Variant)` のいずれかで境界を明示的に処理する必要があります（`|==` は `throw` されたエラーをキャッチする演算子で、Lax には使えません — 詳細は `docs/reference/operators.md`）。

> **補足**: Enum のデフォルトは「最初のバリアントの ordinal」（`01_types.md` 参照）です。Lax[Enum] の `__value` / `__default` はどちらも `Int(0)` に固定され、バリアント定義順が「既定値は何か」を直接示します。

---

## パース結果は Lax で返ります

JSON パースは失敗する可能性があります。`JSON[raw, Schema]()` の戻り値は `Lax[T]` です。

```taida
Pilot = @(name: Str, age: Int, active: Bool)

// パース成功
raw <= '{"name": "Rei", "age": 14, "active": true}'
JSON[raw, Pilot]() ]=> pilot
// pilot: @(name <= "Rei", age <= 14, active <= true)

// パース失敗（不正な JSON）
JSON["invalid json", Pilot]().hasValue    // false
JSON["invalid json", Pilot]() ]=> pilot2
// pilot2: @(name <= "", age <= 0, active <= false)  // 全てデフォルト値
```

### hasValue で成功判定

```taida
result <= JSON[raw, Pilot]()
result.hasValue    // true ならパース成功、false なら失敗
```

### getOrDefault でフォールバック

```taida
fallback <= Pilot(name <= "Unknown", age <= 0, active <= false)
JSON[raw, Pilot]().getOrDefault(fallback)
```

---

## スキーママッチングの 6 つのルール

JSON データをスキーマに照合する際の動作を、6 つのルールで説明します。

### ルール 1: フィールド一致

スキーマに定義されたフィールドのみが抽出されます。余分なフィールドは無視されます。

```taida
Pilot = @(name: Str, age: Int)

raw <= '{"name": "Asuka", "age": 14, "extra": "ignored"}'
JSON[raw, Pilot]() ]=> pilot
// pilot: @(name <= "Asuka", age <= 14)
// "extra" は無視されます
```

### ルール 2: フィールド欠落はデフォルト値

スキーマに定義されているが JSON にないフィールドは、型のデフォルト値になります。

```taida
Pilot = @(name: Str, age: Int, active: Bool)

raw <= '{"name": "Rei"}'
JSON[raw, Pilot]() ]=> pilot
// pilot: @(name <= "Rei", age <= 0, active <= false)
```

### ルール 3: 型不一致はデフォルト値

JSON フィールドの型がスキーマと合わない場合、デフォルト値になります。

```taida
Pilot = @(name: Str, age: Int)

raw <= '{"name": "Asuka", "age": "not a number"}'
JSON[raw, Pilot]() ]=> pilot
// pilot: @(name <= "Asuka", age <= 0)
```

### ルール 4: null はデフォルト値

JSON の `null` は対応する型のデフォルト値に変換されます。Taida に null はありません。

```taida
Pilot = @(name: Str, age: Int, active: Bool)

raw <= '{"name": "Rei", "age": null, "active": null}'
JSON[raw, Pilot]() ]=> pilot
// pilot: @(name <= "Rei", age <= 0, active <= false)
```

### ルール 5: ネストは再帰的にマッチング

ネストしたオブジェクトは、再帰的にスキーママッチングが適用されます。

```taida
Address = @(city: Str, zip: Str)
Pilot = @(name: Str, address: Address)

raw <= '{"name": "Asuka", "address": {"city": "Tokyo-3"}}'
JSON[raw, Pilot]() ]=> pilot
// pilot.address.city: "Tokyo-3"
// pilot.address.zip: ""（欠落 → デフォルト値）
```

### ルール 6: リストは各要素にスキーマを適用

配列の場合、各要素に個別にスキーママッチングが適用されます。

```taida
Pilot = @(name: Str, sync_rate: Int)

raw <= '[{"name": "Rei", "sync_rate": 95}, {"name": "Shinji"}]'
JSON[raw, @[Pilot]]() ]=> pilots
// pilots: @[
//   @(name <= "Rei", sync_rate <= 95),
//   @(name <= "Shinji", sync_rate <= 0)  // sync_rate 欠落 → デフォルト値
// ]
```

---

## 出力方向: jsonEncode / jsonPretty

Taida の値を JSON 文字列に変換するには、プリリュード関数 `jsonEncode` と `jsonPretty` を使います。

出力方向（Taida → 外部）は型安全なので、スキーマは不要です。

```taida
pilot <= @(name <= "Asuka", age <= 14, active <= true)

// コンパクトな JSON 文字列
jsonStr <= jsonEncode(pilot)
// '{"name":"Asuka","age":14,"active":true}'

// 整形された JSON 文字列
prettyStr <= jsonPretty(pilot)
// '{
//   "name": "Asuka",
//   "age": 14,
//   "active": true
// }'
```

### Enum フィールドは variant 名 Str で出力 (C18-2)

`jsonEncode` は Enum 値を宣言した variant 名の Str で出力します（ordinal Int ではありません）。これは `JSON[raw, Schema]()` の decode 側（「Enum 型フィールドの検査」節）と対称な wire 形式で、encode → decode の round-trip が保証されます。

```taida
Enum => HiveState = :Creating :Running :Stopped
Status = @(state: HiveState, count: Int)

rec <= Status(state <= HiveState:Running(), count <= 42)
stdout(jsonEncode(rec))
// {"count":42,"state":"Running"}
```

round-trip の確認:

```taida
// encode
raw <= jsonEncode(rec)

// decode — 同じ schema を噛ませて戻す
JSON[raw, Status]() ]=> rec2
stdout(rec2.state == HiveState:Running())  // true
```

**変換規則**: variant 名はそのまま wire に出ます。snake_case / camelCase への自動変換は行いません。命名は開発者側の責任です。

**既存の ordinal Int 出力が必要な場合**: C18-3 の `Ordinal[]` モールドで Int に明示変換してから encode してください。

```taida
// Int カラムとの互換が必要な場合
payload <= @(state_id <= Ordinal[rec.state]())
stdout(jsonEncode(payload))  // {"state_id":1}
```

---

## 実用パターン

### API レスポンスの処理

```taida
Pilot = @(name: Str, age: Int)

fetchPilot id: Int =
  |== error: Error =
    Pilot(name <= "", age <= 0)
  => :Pilot

  response <= httpGet("https://api.nerv.jp/pilots/" + Str[id]() ]=> _)
  response ]=> res
  JSON[res.body, Pilot]() ]=> pilot
  pilot
=> :Pilot
```

### 設定ファイルの読み込み

```taida
Config = @(host: Str, port: Int)

loadConfig path: Str =
  |== error: Error =
    Config(host <= "localhost", port <= 8080)
  => :Config

  contents <= readFile(path)
  contents ]=> text
  JSON[text, Config]() ]=> config
  config
=> :Config
```

### 同じ JSON から複数の側面を抽出

同じ生データから、異なるスキーマで必要な情報だけを取り出せます。

```taida
UserInfo = @(name: Str, email: Str)
BillingInfo = @(plan: Str, amount: Int)

raw <= fetchJson("/api/account/123")

JSON[raw, UserInfo]() ]=> user
JSON[raw, BillingInfo]() ]=> billing

// 同じ JSON から必要な側面だけを抽出します
stdout(user.name + " is on " + billing.plan + " plan")
```

### パース結果の安全な処理

```taida
Pilot = @(name: Str, age: Int)

processPilotData raw: Str =
  result <= JSON[raw, Pilot]()
  | result.hasValue |> stdout("Pilot: " + result.__value.name)
  | _ |> stderr("Failed to parse pilot data")
=> :Void
```

---

## なぜ溶鉄なのか

### 1. null の完全排除

JSON の `null` は Taida に存在しません。スキーママッチングが `null` を自動的にデフォルト値に変換するため、型安全な世界に `null` が侵入することはありません。

### 2. 型の保証

JSON のフィールドの型は実行時まで不明です。`"age"` が文字列かもしれませんし、オブジェクトかもしれません。スキーマを通して鋳造することで、型が保証されます。

### 3. AI にとっての明確さ

AI がコードを生成する際、「この変数は JSON から来たのか、型安全な値なのか」が常に明確です。JSON 型のまま操作できないため、必ずスキーマを経由した型付き値として扱われます。

### 4. IO 型汚染問題への回答

Haskell は「一度 IO に触れたら IO が伝播する」内から外への汚染伝播型です。Taida は「内部に入るには必ず鋳型を通る」外から内への関所型です。方向が逆であり、外部データの侵入を水際で止めます。

---

## まとめ

JSON 溶鉄の原則:

1. **JSON は不透明なプリミティブ型です** -- メソッドなし、直接操作不可、直接 unmold 不可です
2. **`JSON[raw, Schema]()` がただ一つの入口です** -- スキーマ（鋳型）を通さなければ中身に触れません
3. **戻り値は Lax です** -- パース失敗時は `hasValue = false` で、unmold するとデフォルト値が返ります
4. **スキーママッチングの 6 ルールが動作します** -- フィールド一致、欠落はデフォルト値、型不一致はデフォルト値、null はデフォルト値、ネストは再帰、リストは各要素に適用
5. **Enum 型フィールドは検査されます** -- variant 一致時は ordinal、不一致 / 欠落 / null は `Lax[Enum]`（silent coercion なし）
6. **出力方向は jsonEncode / jsonPretty** -- Taida の型安全な値から JSON 文字列への変換です
7. **JSON パースは `JSON[raw, Schema]()` のみです** -- 他の手段はありません

型安全な世界と外部世界の境界を、溶鉄と鋳型のメタファーで厳格に守ります。
