# 型システム

> **PHILOSOPHY.md -- I.** 深く考えずに適当にぶちこんでけ

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
raw ]=> bytes
bytes.length()                 // 4
bytes.get(0) ]=> b0            // 112
```

**デフォルト値**: `Bytes[@[]]`（空バイト列）

`Bytes` は不変です。更新は `ByteSet[...]()` が新しい値を返します。  
また、`Utf8Encode` / `Utf8Decode` で文字列との変換を行えます。

### Error -- エラー型

全てのエラーの基底型です。

```taida
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

外部から入ってくる不定形データを表す型です。Molten は**溶鉄**です。形がなく、触れません。そのままでは何もできません。型安全な世界に持ち込むには、必ず Cage 経由で鋳型を通す必要があります。

Molten には型パラメータがありません。Molten は Molten でしかありません。メソッドも持たず、直接操作は一切できません。

`Bytes` と `Molten` は役割が異なります。

- `Bytes`: 3バックエンド共通のバイナリデータ型
- `Molten`: JS外部世界由来の不透明値（JS専用連携境界）

`UnsafePointer` はコア言語に導入しません。危険な低レベル境界は `Molten` などの不透明ハンドル + 専用APIで隔離します。

**デフォルト値**: `Molten`（Molten 型のデフォルト値は Molten 自身です。空の溶鉄です）

### JSON -- Molten の特殊ケース

JSON は Molten の特殊ケースです。外部から入ってくる JSON データを表します。JSON は溶鉄であり、形がなく、触れません。使うにはスキーマ（鋳型）を通す必要があります。

```taida
Pilot = @(name: Str, age: Int)

// スキーマを指定して鋳造します
JSON[rawStr, Pilot]() ]=> pilot
pilot.name   // 型安全にアクセスできます
```

JSON にはメソッドがありません。`json.at("name")` のような直接操作はできません。必ず `JSON[raw, Schema]()` でスキーマを通してください。戻り値は `Lax` です。

JSON は `Molten` の一種ですが、専用のモールド `JSON[raw, Schema]()` を持つ点が特殊です。他の Molten 値は `Cage` 経由で操作します。

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

**デフォルト値**: `@[]` (空リスト)

操作はモールドで行います:

```taida
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
@[1, 2, 3].get(0) ]=> val                // 1 (Lax を返します)
@[1, 2, 3].first() ]=> val               // 1 (Lax を返します)
empty: @[Int] <= @[]
empty.first() ]=> val                    // 0 (空リスト: デフォルト値)
```

### ぶちパック `@(...)`

> **PHILOSOPHY.md -- II.** だいじなものはふくろにしまっておきましょう

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

## モールディング型 `Mold[T]`

> **PHILOSOPHY.md -- III.** カタめたいなら、鋳型を作りましょう

型パラメータ化を実現する仕組みです。値を鋳型に流し込み（モールディング）、必要なときに取り出します（アンモールディング `]=>` / `<=[`）。

```taida
// 鋳型の定義
Mold[T] => Result[T, P <= :T => :Bool] = @(throw: Error)  // 述語付き操作モールド
Mold[T] => Lax[T] = @(hasValue: Bool)   // 安全モールド
```

### Result[T, P]

述語 P（`:T => :Bool`）で成功/失敗を判定する操作モールドです。`]=>` で述語を評価し、真なら値 T、偽なら throw が発動します。

```taida
Error => NotFound = @(message: Str)

// _ = true / _ = false は常に真/偽を返す無名関数
ok <= Result[42, _ = true]()
err <= Result[0, _ = false](throw <= NotFound(type <= "NotFound", message <= "fail"))

// => と ]=> の違い
ok ]=> value                  // 42（述語 true → 成功）
err ]=> value                 // throw 発動（エラー天井で捕捉可能）
err => r                      // r: Result[Int, _]（述語は未評価）

// ラムダ述語によるバリデーション
Result[age, _ x = x >= 18](throw <= err) ]=> validAge
```

### Lax[T] -- 必ず値を返すモールド型

「操作が失敗しても必ず値を返す」ことを保証するモールド型です。失敗時は型 T のデフォルト値にフォールバックします。

```taida
// 除算: ゼロ除算でも必ず値を返します
Div[10, 3]() ]=> result   // 3
Div[10, 0]() ]=> result   // 0 (Int のデフォルト値)

// リストアクセス: 範囲外でも必ず値を返します
items <= @[10, 20, 30]
items.get(1) ]=> val       // 20
items.get(100) ]=> val     // 0 (Int のデフォルト値)

// 型変換: 変換失敗でも必ず値を返します
Int["42"]() ]=> num        // 42
Int["abc"]() ]=> num       // 0 (変換失敗: デフォルト値)
```

`hasValue` フィールドで成功/失敗を判別できます:

```taida
lax <= Div[10, 0]()
lax.hasValue               // false (ゼロ除算は失敗)

lax2 <= Div[10, 3]()
lax2.hasValue              // true (正常に割れた)
```

**デフォルト値**: 内包する型 T のデフォルト値

### Gorillax[T] -- 覚悟のモールド型

「操作が失敗したらゴリラがプログラムを止める」モールド型です。Lax のようなデフォルト値へのフォールバックはありません。外部パッケージ（npm 等）の Molten 値への操作で使います。

```taida
// Cage で Molten（溶鉄）に操作を実行 → Gorillax で包まれます
Cage[lodash, _ lo = lo.sum(items)] => rilax  // rilax: Gorillax[Int]
rilax ]=> total              // 成功 → 値, 失敗 → ゴリラ（プログラム終了）

// .relax() で RelaxedGorillax に変換（|== でキャッチ可能に）
rilax.relax() => relaxed     // relaxed: RelaxedGorillax[Int]
```

**デフォルト値**: なし（unmold 失敗時はゴリラまたは throw）

### Async[T]

非同期処理を表現します。`]=>` がブロッキング await として機能します。

```taida
fetchData url: Str =
  httpGet(url)
=> :Async[@(body: Str)]

data <= fetchData("https://api.nerv.jp/pilots")
data ]=> response          // 完了まで待ちます
```

---

## 特殊型: Molten -- 溶鉄

Molten は他の型とは根本的に異なります。溶鉄です。形がなく、メソッドを持たず、直接操作はできません。JSON は Molten の特殊ケースであり、npm パッケージ等から得られる値も Molten です。

### 溶鉄の鋳造

```taida
// スキーマ（鋳型）を定義します
Pilot = @(
  name: Str
  age: Int
  syncRate: Int
)

// JSON を鋳型に流し込みます
raw <= '{"name": "Asuka", "age": 14, "syncRate": 78}'
JSON[raw, Pilot]() ]=> pilot

pilot.name      // "Asuka"
pilot.age       // 14
pilot.syncRate  // 78
```

### スキーマ照合のルール

| ルール | 動作 |
|--------|------|
| フィールド一致 | スキーマに定義されたフィールドのみ抽出。余分は無視 |
| フィールド欠損 | 型のデフォルト値を使用 |
| 型不一致 | 型のデフォルト値を使用 |
| null | 型のデフォルト値に変換（null 排除） |
| ネスト | 再帰的にスキーマ照合 |
| リスト | 各要素にスキーマを適用 |

```taida
// フィールド欠損 → デフォルト値
raw <= '{"name": "Rei"}'
JSON[raw, Pilot]() ]=> pilot
// pilot = @(name <= "Rei", age <= 0, syncRate <= 0)

// null → デフォルト値
raw <= '{"name": null, "age": null}'
JSON[raw, Pilot]() ]=> pilot
// pilot = @(name <= "", age <= 0, syncRate <= 0)
```

### 出力方向は安全

Taida の値を JSON 文字列にする方向は型安全です。プリリュード関数を使います:

```taida
pilot <= Pilot(name <= "Asuka", age <= 14, syncRate <= 78)
jsonEncode(pilot)   // '{"name":"Asuka","age":14,"syncRate":78}'
jsonPretty(pilot)   // 整形された JSON 文字列
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
| JSON | `{}` (Molten の特殊ケース) |
| Lax[T] | T のデフォルト値 |

```taida
// 型を定義すれば、省略されたフィールドはデフォルト値になります
Pilot = @(name: Str, callSign: Str, age: Int)
rei <= Pilot(name <= "Rei")  // callSign = "", age = 0
```

---

## 型変換 -- モールドで明示的に

Taida では暗黙の型変換は一切行われません。型変換はモールドで明示的に行います。全ての型変換モールドは Lax を返します。

```taida
// Str → Int
Int["123"]() ]=> num         // 123
Int["abc"]() ]=> num         // 0 (変換失敗: デフォルト値)

// Str → Int（基数指定）
Int["ff", 16]() ]=> hex      // 255
Int["1010", 2]() ]=> bin     // 10

// Str → Float
Float["3.14"]() ]=> val      // 3.14
Float["abc"]() ]=> val       // 0.0 (変換失敗: デフォルト値)

// Int → Str
Str[42]() ]=> text           // "42"

// 値 → Bool
Bool[1]() ]=> flag           // true
Bool[0]() ]=> flag           // false
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

`.toString()` は **全ての値型で利用できる共通メソッド** です（C12.rc3 で
3 バックエンド全てに正式採用）。Int / Float / Bool / Str / List / ぶちパック /
Lax / Result / HashMap / Set など、どの型に対しても呼び出せて、必ず `:Str`
を返します。文字列連結 (`+`) と組み合わせて使うのが標準的な使い方です:

```taida
status <= 404
msg <= "HTTP Error " + status.toString()
stdout(msg)                  // "HTTP Error 404"
```

引数は受け取りません。`n.toString(16)` のように base / precision を渡そう
とすると、`taida check` が `[E1508] Method 'toString' takes 0 argument(s)`
で拒否します。基数指定が必要な場合は `ToRadix[n, base]()` モールド
（`docs/reference/mold_types.md` 参照）を使います。`ToRadix` は
`Lax[Str]` を返すので、通常は `getOrDefault` で unwrap します:

```taida
ToRadix[255, 16]().getOrDefault("") ]=> hex   // "ff"
ToRadix[26, 2]().getOrDefault("") ]=> bin     // "11010"
```

精度指定など `ToRadix` でカバーできないフォーマットは専用の関数を別途
定義します（哲学 I: 暗黙の型変換なし）。

### 型変換モールド一覧

| モールド | 入力 | 出力 | 説明 |
|---------|------|------|------|
| `Int[x]()` | Str, Float, Bool | Lax[Int] | 整数に変換 |
| `Int[str, base]()` | Str, Int (2..36) | Lax[Int] | 指定基数で文字列を整数に変換 |
| `Float[x]()` | Str, Int | Lax[Float] | 浮動小数点に変換 |
| `Str[x]()` | Int, Float, Bool | Lax[Str] | 文字列に変換 |
| `Bool[x]()` | Int, Str | Lax[Bool] | 真偽値に変換 |

`Int[str]()` は文字列から整数への変換の正規経路です。`"+5"` や `"-7"` のような符号付き文字列も受理します。空文字列、数字以外の文字を含む文字列、小数点を含む文字列は変換失敗（`hasValue=false`）になります。

---

## 型アノテーション

型推論があるので、ほとんどの場合は型アノテーションを書く必要がありません。書きたいときだけ書けば十分です。

```taida
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

## 型エイリアス

型に別名をつけることができます。

```taida
PilotId = Int
PilotName = Str
PilotList = @[Pilot]

id: PilotId <= 2
name: PilotName <= "Asuka"
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

### ビルトイン enum: HttpProtocol

`taida-lang/net` パッケージは `HttpProtocol` enum をエクスポートします:

```taida
>>> taida-lang/net => @(httpServe, HttpProtocol)

// HttpProtocol:H1() → 0 (wire: "h1.1")
// HttpProtocol:H2() → 1 (wire: "h2")
// HttpProtocol:H3() → 2 (wire: "h3")
httpServe(8080, handler, 1, 1000, 128, @(protocol <= HttpProtocol:H1()))
```

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
| プリミティブ | JSON | `{}` (Molten の特殊ケース、メソッドなし) |
| コレクション | @[T] (リスト) | `@[]` |
| コレクション | @(...) (ぶちパック) | 各フィールドのデフォルト値 |
| モールディング | Result[T, P] | T のデフォルト値 |
| モールディング | Lax[T] | T のデフォルト値 |
| モールディング | Async[T] | T のデフォルト値 |
| 列挙型 | Enum | 最初のバリアント (ordinal 0) |

操作はモールドで行います。メソッドは状態チェック + toString + モナディック操作のみです。型変換もモールドで明示的に行います。null はありません。
