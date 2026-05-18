# 型システム

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ

Taida の型システムは、開発者が型を意識しなくて済むように設計されています。強力な型推論があり、全ての型にデフォルト値が保証され、暗黙の型変換は行われません。書くときは何も考えなくて構いません。裏では型チェッカーがガチガチに守っています。

---

## プリミティブ型

### Int -- 整数

```taida
count <= 42
negative <= -10
```

**デフォルト値**: `0`

### Float -- 浮動小数点数

IEEE 754 double precision です。

```taida
pi <= 3.14159
rate <= 0.5
```

**デフォルト値**: `0.0`

`3.0` と `3` は Taida のパーサで `FloatLit` / `IntLit` として区別されます。
バックエンドごとの実行詳細 (WASM プロファイル別の SIMD 設定など) は
[CLI リファレンス](../reference/cli.md) を参照してください。

### Str -- 文字列

```taida
name <= "Rei"
greeting <= 'Hello'
template <= `Hello, ${name}!`
```

**デフォルト値**: `""` (空文字列)

エスケープシーケンス:

| シーケンス | 文字 |
|-----------|------|
| `\n` | 改行 |
| `\t` | タブ |
| `\r` | 復帰 |
| `\\` | バックスラッシュ |
| `\'` | シングルクォート |
| `\"` | ダブルクォート |
| `\0` | null文字 |
| `\xHH` | 16進エスケープ (2桁) |
| `\u{HHHH}` | Unicodeエスケープ (1〜6桁) |

操作はモールドまたはメソッドで行います:

```taida
// モールドで操作
Upper["hello"]()                         // "HELLO"
Trim["  hello  "]()                      // "hello"
Split["a,b,c", ","]()                    // @["a", "b", "c"]
Replace["hello world", "world", "taida"]()  // "hello taida"

// メソッドで操作
"hello world".replace("world", "taida")  // "hello taida" (最初の一致のみ)
"aaa".replaceAll("a", "b")              // "bbb" (全置換)
"a,b,c".split(",")                       // @["a", "b", "c"]

// 状態チェックメソッド
"hello".length()                         // 5
"hello".contains("ell")                  // true
"hello".startsWith("he")                 // true
"hello".toString()                       // "hello"
```

### Bool -- 真偽値

```taida
active <= true
deleted <= false
```

**デフォルト値**: `false`

### Bytes -- バイト列（0..255）

バイナリ境界で使う組み込み型です。`Bytes` は `@[Int]` と違い、`0..255` の連続領域を表す用途に固定されています。

```taida
raw <= Bytes["ping"]()
raw >=> bytes
bytes.length()                 // 4
bytes.get(0) >=> b0            // 112
```

**デフォルト値**: `Bytes[@[]]`（空バイト列）

`Bytes` は不変です。更新は `ByteSet[...]()` が新しい値を返します。  
また、`Utf8Encode` / `Utf8Decode` で文字列との変換を行えます。

### Error -- エラー型

全てのエラーの基底型です。

```taida fragment
Error = @(
  type: Str
  message: Str
)
```

カスタムエラーは Error を継承して定義します:

```taida
Error => ValidationError = @(
  field: Str
  code: Int
)
```

### Molten -- 溶鉄（不透明プリミティブ）

外部から入ってくる不定形データを表す型です。Molten は**溶鉄**です。形がなく、触れません。そのままでは何もできません。

Molten には型パラメータがありません。Molten は Molten でしかありません。メソッドも持たず、直接操作は一切できません。

Molten は型引数を持たない基底であり、用途別に次の分岐へ分かれます:

- `Molten => JSON`: JSON のデコード専用。`JSON[raw, Schema]()` で型安全な値へ鋳造します。
- `Molten => JS`: JS / npm 連携専用。`Cage` と JS 補助モールドはこの分岐だけを扱います。
- `Molten => TemperedMolten[T]`: ビルド記述子向けの分岐。ランタイムから直接は利用しません。

スキーマ照合の挙動と JSON 出力方向の安全性は [JSON 溶鉄](03_json.md) を参照してください。

`Bytes` と `Molten` は役割が異なります。

- `Bytes`: バイナリデータを保持するバックエンド共通の組み込み型
- `Molten`: 外部世界（JS / JSON など）由来の不透明値で、スキーマを通さなければ使えません

`UnsafePointer` はコア言語に導入しません。危険な低レベル境界は `Molten` などの不透明ハンドル + 専用 API で隔離します。

**デフォルト値**: `Molten`（空の溶鉄）

### JSON -- Molten の JSON 分岐

JSON は `Molten => JSON` の分岐です。外部から入ってくる JSON データを表します。JSON は溶鉄であり、形がなく、触れません。使うにはスキーマ（鋳型）を通す必要があります。

```taida
Pilot = @(name: Str, age: Int)

// スキーマを指定して鋳造します
JSON[rawStr, Pilot]() >=> pilot
pilot.name   // 型安全にアクセスできます
```

JSON にはメソッドがありません。`json.at("name")` のような直接操作はできません。必ず `JSON[raw, Schema]()` でスキーマを通してください。戻り値は `Lax` です。

JSON は `Molten[Str]` のような型引数つきの Molten ではありません。必ず専用のモールド
`JSON[raw, Schema]()` を使います。`Cage` は JSON 分岐を受け取らず、JS 分岐
(`Molten => JS`) だけを扱います。

**デフォルト値**: `{}` (空オブジェクト)

---

## コレクション型

### リスト `@[T]`

同種の値の列です。

```taida
numbers <= @[1, 2, 3, 4, 5]
names <= @["Asuka", "Rei"]
empty: @[Int] <= @[]
```

空リスト束縛では要素型を必ず型注釈で示してください。`empty <= @[]` のように省略すると要素型が確定せず、後続の要素操作で型エラー（`[E0401]`）の原因になります。

**デフォルト値**: `@[]` (空リスト)

操作はモールドで行います:

```taida fragment
// モールドで操作します
Sort[@[3, 1, 2]]()                       // @[1, 2, 3]
Filter[@[1, 2, 3, 4], _ x = x > 2]()    // @[3, 4]
Map[@[1, 2, 3], _ x = x * 2]()          // @[2, 4, 6]
Concat[@[1, 2], @[3, 4]]()              // @[1, 2, 3, 4]
Join[@["a", "b", "c"], ","]()            // "a,b,c"

// パイプラインで連鎖します
numbers => Filter[_, _ x = x > 2]() => Map[_, _ x = x * 10]() => result

// メソッドは状態チェックと Lax 返しのみです
@[1, 2, 3].length()                      // 3
@[1, 2, 3].isEmpty()                     // false
@[1, 2, 3].contains(2)                   // true
@[1, 2, 3].get(0) >=> val                // 1 (Lax を返します)
@[1, 2, 3].first() >=> val               // 1 (Lax を返します)
empty: @[Int] <= @[]
empty.first() >=> val                    // 0 (空リスト: デフォルト値)
```

### ぶちパック `@(...)`

> **PHILOSOPHY.md — II.** だいじなものはふくろにしまっておきましょう

名前付きフィールドの集合です。

```taida
pilot <= @(name <= "Shinji", age <= 14)

// 型定義
Pilot = @(
  name: Str
  age: Int
  active: Bool
)

asuka <= Pilot(name <= "Asuka", age <= 14, active <= true)
```

**デフォルト値**: 各フィールドのデフォルト値

---

## モールド型 `Mold[T]`

> **PHILOSOPHY.md — III.** カタめたいなら、鋳型を作りましょう

型パラメータ化を実現する仕組みです。値を鋳型に流し込み（モールド）、必要なときに取り出します（アンモールド `>=>` / `<=<`）。

```taida
// 鋳型の定義
Mold[T] => Result[T, P <= :T => :Bool] = @(throw: Error)  // 述語付き操作モールド
Mold[T] => Lax[T] = @(has_value: Bool)   // 安全モールド
```

### Result[T, P]

述語 P（`:T => :Bool`）で成功/失敗を判定する操作モールドです。`>=>` で述語を評価し、真なら値 T、偽なら throw が発動します。

```taida fragment
Error => NotFound = @(message: Str)

// _ = true / _ = false は常に真/偽を返す無名関数
ok <= Result[42, _ = true]()
err <= Result[0, _ = false](throw <= NotFound(type <= "NotFound", message <= "fail"))

// => と >=> の違い
ok >=> value                  // 42（述語 true → 成功）
err >=> value                 // throw 発動（エラー天井で捕捉可能）
err => r                      // r: Result[Int, _]（述語は未評価）

// ラムダ述語によるバリデーション
Result[age, _ x = x >= 18](throw <= err) >=> validAge
```

### Lax[T] -- 必ず値を返すモールド型

「操作が失敗しても必ず値を返す」ことを保証するモールド型です。失敗時は型 T のデフォルト値にフォールバックします。

```taida fragment
// 除算: ゼロ除算でも必ず値を返します
Div[10, 3]() >=> result   // 3
Div[10, 0]() >=> result   // 0 (Int のデフォルト値)

// リストアクセス: 範囲外でも必ず値を返します
items <= @[10, 20, 30]
items.get(1) >=> val       // 20
items.get(100) >=> val     // 0 (Int のデフォルト値)

// 型変換: 変換失敗でも必ず値を返します
Int["42"]() >=> num        // 42
Int["abc"]() >=> num       // 0 (変換失敗: デフォルト値)
```

`has_value` フィールドで成功/失敗を判別できます:

```taida
lax <= Div[10, 0]()
lax.has_value               // false (ゼロ除算は失敗)

lax2 <= Div[10, 3]()
lax2.has_value              // true (正常に割れた)
```

**デフォルト値**: 内包する型 T のデフォルト値

### Gorillax[T] -- 覚悟のモールド型

「操作が失敗したらゴリラがプログラムを止める」モールド型です。Lax のようなデフォルト値へのフォールバックはありません。外部パッケージ（npm 等）の Molten 値への操作で使います。

```taida fragment
// Cage で Molten（溶鉄）に branch operation を実行 → Gorillax で包まれます
Cage[lodash, JSCall[@["sum"], @[items], Int]()]() => rilax  // rilax: Gorillax[Int]
rilax >=> total              // 成功 → 値, 失敗 → ゴリラ（プログラム終了）

// .relax() で RelaxedGorillax に変換（|== でキャッチ可能に）
rilax.relax() => relaxed     // relaxed: RelaxedGorillax[Int]
```

**デフォルト値**: なし（アンモールド失敗時はゴリラまたは throw）

### Async[T]

非同期処理を表現します。`>=>` がブロッキング await として機能します。

```taida
fetchData url: Str =
  httpGet(url)
=> :Async[@(body: Str)]

data <= fetchData("https://api.nerv.jp/pilots")
data >=> response          // 完了まで待ちます
```

---

## デフォルト値の完全保証

全ての型にデフォルト値があります。null も undefined も存在しません。

| 型 | デフォルト値 |
|----|-------------|
| Int | `0` |
| Float | `0.0` |
| Str | `""` |
| Bool | `false` |
| @[T] (リスト) | `@[]` |
| @(...) (ぶちパック) | 各フィールドのデフォルト値 |
| Molten | `Molten`（空の溶鉄） |
| JSON 分岐 | `{}` |
| Lax[T] | T のデフォルト値 |

```taida
// 型を定義すれば、省略されたフィールドはデフォルト値になります
Pilot = @(name: Str, call_sign: Str, age: Int)
rei <= Pilot(name <= "Rei")  // call_sign = "", age = 0
```

---

## 型変換 -- モールドで明示的に

Taida では暗黙の型変換は一切行われません。型変換はモールドで明示的に行います。全ての型変換モールドは Lax を返します。

```taida fragment
// Str → Int
Int["123"]() >=> num         // 123
Int["abc"]() >=> num         // 0 (変換失敗: デフォルト値)

// Str → Int（基数指定）
Int["ff", 16]() >=> hex      // 255
Int["1010", 2]() >=> bin     // 10

// Str → Float
Float["3.14"]() >=> val      // 3.14
Float["abc"]() >=> val       // 0.0 (変換失敗: デフォルト値)

// Int → Str
Str[42]() >=> text           // "42"

// 値 → Bool
Bool[1]() >=> flag           // true
Bool[0]() >=> flag           // false
```

`toString()` メソッドも使えます。ただし型変換モールドと違い、Lax ではなく Str を直接返します:

```taida
42.toString()                // "42"
3.14.toString()              // "3.14"
true.toString()              // "true"
"hello".toString()           // "hello" (identity)
@[1, 2, 3].toString()        // "@[1, 2, 3]"
@(a <= 1, b <= 2).toString() // "@(a <= 1, b <= 2)"
```

`.toString()` は **全ての値型で利用できる共通メソッド** です。Int / Float /
Bool / Str / List / ぶちパック / Lax / Result / HashMap / Set など、どの型に
対しても呼び出せて、必ず `:Str` を返します。文字列連結 (`+`) と組み合わせて
使うのが標準的な使い方です:

```taida
status <= 404
msg <= "HTTP Error " + status.toString()
stdout(msg)                  // "HTTP Error 404"
```

引数は受け取りません。`n.toString(16)` のように base / precision を渡そう
とすると、`taida way check` が `[E1508] Method 'toString' takes 0 argument(s)`
で拒否します。基数指定が必要な場合は `ToRadix[n, base]()` モールド
（[`docs/api/prelude.md §7.1`](../api/prelude.md#71-数値拡張モールド) 参照）を使います。`ToRadix` は
`Lax[Str]` を返すので、通常は `getOrDefault` で unwrap します:

```taida
ToRadix[255, 16]().getOrDefault("") >=> hex   // "ff"
ToRadix[26, 2]().getOrDefault("") >=> bin     // "11010"
```

精度指定など `ToRadix` でカバーできないフォーマットは専用の関数を別途
定義します（哲学 I: 暗黙の型変換なし）。

### 型変換モールド一覧

| モールド | 入力 | 出力 | 説明 |
|---------|------|------|------|
| `Int[x]()` | Str, Float, Bool | Lax[Int] | 整数に変換 |
| `Float[x]()` | Str, Int | Lax[Float] | 浮動小数点に変換 |
| `Str[x]()` | Int, Float, Bool | Lax[Str] | 文字列に変換 |
| `Bool[x]()` | Int, Str | Lax[Bool] | 真偽値に変換 |

`Int` モールドだけは引数の数を 1 個または 2 個から選べます。1 引数なら 10 進数として解釈し、2 引数なら第二引数で基数 (2〜36) を指定します。

```taida fragment
Int["42"]() >=> dec           // 42 (10 進数として解釈)
Int["ff", 16]() >=> hex       // 255 (16 進数として解釈)
Int["1010", 2]() >=> bin      // 10 (2 進数として解釈)
```

`Int` は文字列から整数への変換の正規経路です。`"+5"` や `"-7"` のような符号付き文字列も受理します。空文字列、数字以外の文字を含む文字列、小数点を含む文字列は変換失敗（`has_value=false`）になります。

---

## 型アノテーション

型推論があるので、変数束縛ではほとんどの場合に型アノテーションを書く必要がありません。書きたいときだけ書けば十分です。ただし**関数定義の戻り型 (`=> :T`) は必須**で、省略するとパースエラーになります。

```taida fragment
// 型推論に任せます（ほとんどの場合はこれで十分です）
x <= 42
name <= "Asuka"
pilots <= @[Pilot(name <= "Ritsuko")]

// 明示的に型を指定します
x: Int <= 42
name: Str <= "Rei"
pilots: @[Pilot] <= @[Pilot(name <= "Ritsuko")]
```

### 関数の型指定

関数の戻り型 (`=> :T`) は省略できません。引数の型は推論が効く文脈であれば省略可能ですが、ガイドラインとしては明示する方を推奨します。

```taida
add x: Int y: Int =
  x + y
=> :Int

createPilot name: Str age: Int =
  @(name <= name, age <= age, active <= true)
=> :@(name: Str, age: Int, active: Bool)
```

### 関数型シグネチャ

```taida
// 関数型は `:引数型 => :戻り型` の形式です
:Int => :Str      // Int から Str への関数
:Int :Int => :Int  // Int, Int から Int への関数
```

---

## 型推論

Hindley-Milner ベースの型推論により、大部分の場合で型アノテーションは不要です。

```taida
// リテラルからの推論
num <= 42              // Int
text <= "hello"        // Str
flag <= true           // Bool
list <= @[1, 2, 3]     // @[Int]

// 演算結果からの推論
sum <= 1 + 2           // Int
concat <= "a" + "b"    // Str

// ぶちパックからの推論
pilot <= @(name <= "Rei", age <= 14)
// pilot: @(name: Str, age: Int)

// モールドの結果からの推論
upper <= Upper["hello"]()    // Str
divided <= Div[10, 3]()      // Lax[Int]
```

---

## 構造的部分型付け

Taida は構造的部分型付けを採用しています。必要なフィールドを持っていれば、余分なフィールドがあっても互換性があります。

```taida
Pilot = @(
  name: Str
  age: Int
)

NervStaff = @(
  name: Str
  age: Int
  department: Str
  rank: Int
)

greet pilot: Pilot =
  "Hello, " + pilot.name
=> :Str

staff <= NervStaff(name <= "Ritsuko", age <= 30, department <= "Science", rank <= 2)
message <= greet(staff)  // OK: NervStaff は Pilot の部分型です
```

---

## Enum 型

列挙型です。有限個のバリアントから1つを選ぶ値を定義します。

### 定義

```taida
Enum => Status = :Ok :Fail :Retry
```

`Enum =>` でenum定義を開始し、バリアントを `:名前` で列挙します。

### 使用

```taida
Enum => Status = :Ok :Fail :Retry

myStatus <= Status:Retry()
stdout(myStatus)              // 2（ordinal値、0-indexed）
stdout(myStatus == Status:Retry())  // true
```

enum値は ordinal（0始まりの Int）として評価されます。`Status:Ok()` は `0`、`Status:Fail()` は `1`、`Status:Retry()` は `2` です。

### モジュール越境

Enum 型は `<<< @(...)` でエクスポート、`>>> ./mod.td => @(...)` でインポートできます。インポート先では `Status:Ok()` を直接呼び出せます。

```taida
// status.td
Enum => Status = :Ok :Fail :Retry
<<< @(Status)

// main.td
>>> ./status.td => @(Status)

s <= Status:Ok()
stdout(s)  // 0
```

**順序整合性**: インポート先で同名の `Enum => Status = ...` を再定義している場合、バリアントの並びがインポート元と一致している必要があります。不整合があると `[E1618]` で拒否されます。

```
[E1618] Enum 'Status' variant order mismatch across module boundary.
```

バリアントの宣言順は意味を持ちます。インポート元で順序を変更すると、既存の呼び出し箇所の ordinal が暗黙に変わり、`jsonEncode` 出力や順序比較の挙動に影響するため、変更時は依存先をすべて確認してください。

### 順序比較

同一 Enum のバリアント同士は、宣言順を使って比較できます。

```taida
Enum => HiveState = :Creating :Running :Stopped

a <= HiveState:Creating()
b <= HiveState:Running()
stdout((a < b).toString())   // true — Creating(0) < Running(1)
stdout((b >= a).toString())  // true

ready s: HiveState =
  | s >= HiveState:Running() |> "yes"
  | _ |> "no"
=> :Str
```

許可される比較:
- 同一 Enum 同士: `HiveState:A() < HiveState:B()`

`[E1605]` で拒否される比較:
- 異なる Enum 同士: `HiveState:A() < OtherEnum:B()`
- Enum と Int: `HiveState:A() > 0` — 明示的な Int 変換が必要

Enum と Int を比較したい場合は、次節の `Ordinal[]` モールドで Int 側に揃えてください。

### `Ordinal[]` モールド

`Ordinal[Enum:Variant()]()` で Enum 値を宣言 ordinal の Int に変換します。

```taida
Enum => HiveState = :Creating :Running :Stopped

n <= Ordinal[HiveState:Running()]()
stdout(n.toString())  // 1

// Int 列との比較は順序比較ではなく Ordinal[] 経由で揃える。
state <= HiveState:Stopped()
ok <= Ordinal[state]() > 0
```

`Ordinal[]` は Enum → Int の唯一の正規経路です。`.toString()` の戻り値を `Int[]` でパースする回避策は将来の仕様変更で壊れるため使わないでください。

### JSON wire 形式

`jsonEncode` は Enum フィールドをバリアント名の文字列として出力します（`JSON[raw, Schema]()` デコーダーと対称）。

```taida
Enum => HiveState = :Creating :Running :Stopped

rec <= @(state <= HiveState:Running())
stdout(jsonEncode(rec))  // {"state":"Running"}
```

詳細は [JSON 溶鉄](03_json.md) を参照してください。

**デフォルト値**: 最初のバリアント（ordinal 0）

---

## まとめ

| 分類 | 型 | デフォルト値 |
|------|-----|-------------|
| プリミティブ | Int | `0` |
| プリミティブ | Float | `0.0` |
| プリミティブ | Str | `""` |
| プリミティブ | Bool | `false` |
| プリミティブ | Molten | `Molten`（空の溶鉄、メソッドなし） |
| プリミティブ | JSON 分岐 | `{}` (メソッドなし) |
| コレクション | @[T] (リスト) | `@[]` |
| コレクション | @(...) (ぶちパック) | 各フィールドのデフォルト値 |
| モールド | Result[T, P] | T のデフォルト値 |
| モールド | Lax[T] | T のデフォルト値 |
| モールド | Async[T] | T のデフォルト値 |
| 列挙型 | Enum | 最初のバリアント (ordinal 0) |

操作はモールドで行います。メソッドは状態チェック + toString + モナディック操作のみです。型変換もモールドで明示的に行います。null はありません。
