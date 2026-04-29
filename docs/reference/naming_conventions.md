# 命名規則

Taida Lang はカテゴリ別命名規則を採用します。本書はその正式仕様です。`taida way lint` (D28B-008、診断コード `E18xx`) が CI で違反を pin します。

## 7 カテゴリ命名規則

| 種別 | 規則 | 例 | 備考 |
|------|------|-----|------|
| クラスライク型 | PascalCase | `Pilot`, `HttpRequest`, `Result` | |
| モールド型 | PascalCase | `Map`, `Filter`, `Zip`, `Enumerate`, `Str`, `Take` | mold 形は **PascalCase で正** |
| スキーマ | PascalCase | `User`, `Request`, `HttpProtocol` | |
| 関数 | camelCase | `httpServe`, `readBytes`, `zip`, `enumerate`, `strOf` | function 形は **camelCase で正** |
| ぶちパックフィールド (関数値) | camelCase | `@(handler <= myFunc)` | 値が関数の場合 |
| ぶちパックフィールド (非関数値) | snake_case | `@(user_name <= "...", port_count <= 8080)` | 値が関数以外 |
| 変数 (関数値の束縛) | camelCase | `processRequest <= ...` | |
| 変数 (非関数値) | snake_case | `user_name`, `port_count` | |
| 定数 | SCREAMING_SNAKE_CASE | `MAX_BUFFER_SIZE`, `DEFAULT_TIMEOUT` | |
| エラー variant | PascalCase | `NotFound`, `Timeout`, `RelaxedGorillaEscaped` | エラー型は型扱い |

> **重要**: モールド形 (PascalCase の `Map`, `Filter` 等) と関数形 (camelCase の `zip`, `enumerate` 等) は **両方 valid で共存します**。種別が異なる (モールド型 vs 関数) ため、規則上は両者が正しく並立します。

## 型変数規則

Taida には subtyping ベースの型制約システムがあります (`T <= :Type`、`P <= :T => :Bool`)。**意味は型変数名ではなく `<=` subtype 制約で表現する**設計です。

- 型変数は **単一大文字** を使用: `T`, `U`, `V`, `E`, `K`, `P`, `R`, `A`, `B`
- **PascalCase の名前付き型変数 (`Item`, `Key`, `Value`) は禁止** — クラスライク型 / モールド型 / スキーマと区別を保つため
- 添字付き形 (`T1`, `T2`, `T3`) は 4 つ以上で衝突する場合のみ許容
- 慣習推奨命名:
  - `T` 汎用 (1 つ目)
  - `U` 2 つ目 / Output
  - `V` 3 つ目 / Value
  - `E` Error
  - `K` Key
  - `P` Predicate
  - `R` Return
  - `A` / `B` 汎用ペア

```taida
// 単純な型変数
Mold[T] => Box[T] = @(value: T)

// subtype 制約で意味を表現
Mold[T <= :Num] => NumBox[T <= :Num] = @()      // Int / Float 両方 valid
Mold[T <= :User] => UserMold[T <= :User] = @()   // structural width subtyping

// 関数型制約 (前方参照型変数)
Mold[T] => Result[T, P <= :T => :Bool] = @(throw: Error)

// 子で新規 slot を末尾追加
Mold[T] => Pair[T, U] = @(second: U)

// NG: PascalCase 名前付き型変数
// Mold[Item] => Box[Item] = @(value: Item)  // [E1807] 違反
```

### header arity / 制約配置のルール

| 操作 | 可否 | 根拠 |
|------|------|------|
| root の arity 増加 (`Mold[T, U] => ...`) | NG (`[E1407]`) | root の `mold_args.len() == 1` 強制 |
| root に制約付け (`Mold[T <= :Int] => Foo[T <= :Int]`) | OK | root と child の slot 完全一致 |
| child で arity 増加 (`Mold[T] => Pair[T, U]`) | OK | 末尾 slot 追加は自由 |
| child で既存 slot に制約後付け (`Mold[T] => Foo[T <= :Int]`) | NG (`[E1407]`) | constraint 込み exact match を要求 |
| child の新規 slot に制約 (`Mold[T] => Guard[T, P <= :T => :Bool]`) | OK | 末尾追加 slot は自由 |

つまり「**制約を書きたいなら root から書く**」「**root の arity は 1 固定で増やせない**」が Taida の設計です。

## 補助規則

- `.td` ファイル名: **snake_case** (例: `net_http_hello.td`、`pilot_service.td`)
- モジュール import: **kebab-case/kebab-case** (GitHub URL 準拠、例: `taida-lang/os`、`taida-lang/net`)
  - 単一関数パッケージで `kebab-case/packageName` (例: `someorg/dateFormat`) も自然なため厳密化しない
- 具象型注釈: **`:` prefix** (`:Int`, `:Str`, `:T => :Bool`)
- 戻り値型注釈: **`=> :Type`** (`:` マーカー必須)。`=> Type` のように `:` を欠いた形は parser が lenient に受理しますが、lint で `[E1809]` を発射します
- 引数 / フィールド型注釈の形式 A (`arg: Type` コロン分離) と 形式 B (`arg :Type` スペース分離) は **どちらも valid** で lint 対象外 — 後置型注釈言語との親和性のため形式 A も maintain

## 触らない項目 (慣習として開放)

以下は CI lint の対象外です:

- テスト関数命名 (関数規則 camelCase を踏襲する以上の規則は設けない)
- `_` prefix (public/protected/private 概念が無いため特別扱いしない、慣習として残す)
- boolean プレフィックス (`is`, `has`, `can`, `did`, `needs` 等を多様性として許容、規約化しない)

## 例

### 型 (PascalCase)

```taida
// 基本的な型定義
Pilot = @(
  name: Str,
  age: Int
)

// モールディング型
Mold[T] => Result[T, P <= :T => :Bool] = @(throw: Error)

// エラー型 (Error を継承)
Error => ValidationError = @(field: Str)
Error => HttpError = @(status_code: Int)
```

### 関数 (camelCase)

```taida
// 基本的な関数
getPilotName pilot: Pilot =
  pilot.first_name + " " + pilot.last_name
=> :Str

// 複数単語の関数名
calculateTotalPrice items: @[Item] =
  Fold[0, items, _ acc item = acc + item.price]() ]=> total
  total
=> :Int

// 真偽値を返す関数 (is/has/can プレフィックスは慣習として開放)
isValidEmail email: Str =
  // 検証ロジック
=> :Bool
```

### 変数 — 値の種別で使い分け

```taida
// 非関数値 (snake_case)
pilot_name <= "Misato"
total_count <= 42
is_active <= true
port_count <= 8080

// 関数値 (camelCase)
processRequest <= _ req = req.method
validateEmail <= isValidEmail
```

### ぶちパックフィールド — 値の種別で使い分け

```taida
// 非関数値フィールドは snake_case
pilot <= @(
  first_name <= "Misato",
  last_name <= "Katsuragi",
  call_sign <= "Ops-01"
)

// 関数値フィールドは camelCase
config <= @(
  handler <= myFunc,
  validator <= _ x = x > 0
)

// 混在 OK
api <= @(
  base_url <= "https://api.example.com",   // snake_case (Str)
  timeout_ms <= 5000,                       // snake_case (Int)
  onError <= _ err = stdout(err.message)    // camelCase (関数値)
)
```

### 定数 (SCREAMING_SNAKE_CASE)

```taida
MAX_RETRY_COUNT <= 3
DEFAULT_TIMEOUT <= 5000
PI <= 3.14159

API_BASE_URL <= "https://api.example.com"
DEFAULT_LOCALE <= "ja-JP"

<<< @(MAX_RETRY_COUNT, DEFAULT_TIMEOUT, API_BASE_URL)
```

### エラー variant / Enum variant (PascalCase)

```taida
Enum => HttpStatus = :Ok :NotFound :ServerError

Error => ApiError = @(reason: Str)
ApiError => NotAuthorized = @()
ApiError => RateLimited = @(retry_after: Int)
```

## モジュール / ファイル名 (snake_case + kebab-case)

```taida
// ファイル: net_http_hello.td
>>> ./http_client.td => @(httpGet, httpPost)
>>> ./json_parser.td => @(parseJson)

PilotService = @(
  getPilot id: Int =
    // 実装
  => :Pilot
)

<<< @(PilotService)
```

### import / export

import / export ではモジュール path は **kebab-case** (GitHub URL 準拠):

```taida
>>> taida-lang/os => @(readBytes, writeBytes)
>>> taida-lang/net => @(httpServe, httpRequest)
>>> alice-lang/utils => @(parse)
```

## 識別子に使用可能な文字

- 英字 (a-z, A-Z)
- 数字 (0-9) ※先頭以外
- アンダースコア (`_`)
- Unicode 文字 (対応予定)

```taida
// OK
pilot_name <= "Misato"
pilot2 <= getPilot(2)
_internal <= "internal"   // _ prefix は lint 対象外 (慣習として開放)

// NG (数字で始まる)
// 2nd_pilot <= ...   // コンパイルエラー
```

## 予約語

Taida Lang には予約語がありません。`unmold`、`throw` なども関数として扱われます。

```taida
// これらは予約語ではなく、関数 / メソッドとして動作
opt.unmold()
error.throw()
```

## CI lint との関係

`taida way lint <PATH>` は本書の規則を E18xx 診断コードで pin します。CI では `lint` job で `taida way lint` を hard-fail (非 0 終了で job fail) として実行します。

| コード | 違反 |
|--------|------|
| `E1801` | クラスライク型 / モールド型 / スキーマ / エラー variant が PascalCase でない |
| `E1802` | 関数が camelCase でない |
| `E1803` | 関数値を束縛する変数が camelCase でない |
| `E1804` | 非関数値を束縛する変数が snake_case でない |
| `E1805` | (reserved) 定数が SCREAMING_SNAKE_CASE でない — usage tracking 後段に予約 |
| `E1806` | エラー variant / Enum variant が PascalCase でない |
| `E1807` | 型変数が単一大文字でない |
| `E1808` | ぶちパックフィールドの値型と命名規則が不整合 |
| `E1809` | 戻り値型注釈の `:` マーカー欠落 |

詳細は `docs/reference/diagnostic_codes.md` を参照してください。

## 推奨事項 (規約化しない、慣習)

### 意味のある名前を使う

```taida
// Good
pilot_count <= pilots.length
active_pilots <= Filter[pilots, _ p = p.is_active]()

// Bad
n <= pilots.length
x <= Filter[pilots, _ p = p.is_active]()
```

### 略語は避ける

```taida
// Good
getPilotById id: Int = ...
calculateTotalPrice items: @[Item] = ...

// Bad
getPltById id: Int = ...
calcTotPrc items: @[Item] = ...
```

### 真偽値には is / has / can プレフィックス (慣習)

```taida
is_valid <= validateInput(data)
has_permission <= checkPermission(pilot)
can_proceed <= is_valid && has_permission
```
