# モジュールシステム

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ

モジュール管理は2つの演算子だけです。`>>>` がインポート、`<<<` がエクスポートです。よく使う関数はプリリュードとして最初から使えます。

---

## プリリュード

インポート不要で、どのファイルからでも使える機能です。2つの層があります。

### 言語組込み（Layer 0）

演算子や構文と同じレベルで組み込まれています。モールドとして提供されます。

| カテゴリ | 例 |
|---------|-----|
| HOF モールド | `Map`, `Filter`, `Fold`, `Foldr`, `Reduce`, `Take`, `TakeWhile`, `Drop`, `DropWhile` |
| 文字列モールド | `Upper`, `Lower`, `Trim`, `Split`, `Replace`, `Slice`, `CharAt`, `Repeat`, `Reverse`, `Pad` |
| 数値モールド | `ToFixed`, `Abs`, `Floor`, `Ceil`, `Round`, `Truncate`, `Clamp` |
| リストモールド | `Concat`, `Append`, `Prepend`, `Join`, `Sum`, `Sort`, `Unique`, `Flatten`, `Find`, `FindIndex`, `Count`, `Zip`, `Enumerate` |
| 型変換モールド | `Str[x]()`, `Int[x]()`, `Float[x]()`, `Bool[x]()` |
| 演算モールド | `Div[x, y]()`, `Mod[x, y]()` |
| JSON モールド | `JSON[raw, Schema]()` |
| 非同期モールド | `Async[T]`, `All`, `Race`, `Timeout` |
| モールディング型 | `Result[T, P]`, `Lax[T]`, `Gorillax[T]`, `Cage[Molten, F]` |
| JS 補助モールド | `JSNew`, `JSSet`, `JSBind`, `JSSpread`（JS バックエンド専用） |

詳細は [モールディング型](05_molding.md) を参照してください。

### プリリュード関数（Layer 1）

| 関数 | 説明 |
|------|------|
| `stdout(value)` | 標準出力に出力します |
| `stderr(value)` | 標準エラー出力に出力します |
| `stdin()` | 標準入力から1行読み取ります |
| `jsonEncode(value)` | 値を JSON 文字列に変換します |
| `jsonPretty(value)` | 値を整形された JSON 文字列に変換します |
| `debug(value)` | 値をそのまま出力します。`debug(value, "label")` でラベル付き出力 |
| `typeof(x)` | 型名を文字列で返します |
| `range(start, end)` | 整数リストを生成します |
| `enumerate(list)` | インデックス付きリストを返します |
| `zip(a, b)` | 2つのリストを組み合わせます |
| `assert(cond, msg)` | 条件が false なら throw します |
| `hashMap(entries)` | HashMap を生成します |
| `setOf(items)` | Set を生成します |

```taida
// import は不要です
stdout("Hello, World!")
stderr("Error occurred")

input <= stdin()
stdout("You typed: " + input)

pilot <= @(name <= "Rei", age <= 14)
json <= jsonEncode(pilot)
stdout(json)  // {"name":"Rei","age":14}

debug(pilot, "check")  // [check] BuchiPack: @(name <= "Rei", age <= 14)
typeof(42)              // "Int"
```

---

## インポート `>>>`

### ローカルモジュール

`>>> ./path.td => @(symbols)` でローカルファイルからシンボルをインポートします。

```taida
// 同じディレクトリのファイルから
>>> ./utils.td => @(helper, format)

// 親ディレクトリのファイルから
>>> ../shared/common.td => @(constants)
```

### シンボル指定の省略

`=> @(symbols)` を省略すると、対象モジュールがエクスポートした全シンボルをインポートします。

```taida
>>> ./utils.td                    // utils.td の全エクスポートを取り込む
>>> ./utils.td => @(helper)       // helper だけを取り込む
```

明示的にシンボルを指定することを推奨します。省略すると名前衝突のリスクがあります。

### エイリアス

インポート時にシンボルの名前を変更できます。

```taida
>>> ./math.td => @(add, subtract => sub)
// subtract を sub として使用できます
result <= sub(10, 3)  // 7
```

### npm パッケージ（JS バックエンドのみ）

`>>> npm:package => @(symbols)` で npm パッケージをインポートします。JS バックエンドでのみ動作します。

npm からインポートされた値は **Molten** 型です。Molten は外部由来の不透明値であり、直接操作はできません。型安全な値を取り出すには Cage 経由で操作します。

```taida
>>> npm:express => @(express)  // express: Molten

// Cage 内で Molten の関数を呼び出します（直接呼び出しは不可）
Cage[express, _ e = e()]() ]=> app       // app: Molten（express() の結果）

// JS 補助モールドは Cage 外でも使えます（Taida のモールド構文）
JSNew[express.Router, @()]() => router   // router: Molten

// Cage で Molten から Taida の型世界に値を持ち込みます
Cage[app, _ a = a.listen(3000)]() => result  // result: Gorillax
result ]=> server                            // server: 値（またはゴリラ）
```

**型の流れ:**

```
npm import (Molten) → JSNew 等 (Molten→Molten) → Cage (Molten→Gorillax) → ]=> (値)
```

インタプリタおよび Native バックエンドで `npm:` インポートを使用するとコンパイルエラーになります。JS 補助モールド（JSNew, JSSet, JSBind, JSSpread）も同様です。

### 外部パッケージ

```taida
>>> author/package => @(funcA, funcB)
```

### 使用例

```taida
>>> ./utils.td => @(helper, double)

result <= double(8)       // 16
processed <= helper(data)
stdout(result)
```

---

## エクスポート `<<<`

### 複数シンボル

```taida
add x: Int y: Int = x + y => :Int
subtract x: Int y: Int = x - y => :Int
Point = @(x: Int, y: Int)

<<< @(add, subtract, Point)
```

### 複数行エクスポート

通常の `.td` ファイルでは `<<<` を複数行に分けて書くことができます。

```taida
<<< @(add, subtract)
<<< @(Point)
```

これは1行にまとめた場合と同じ効果です。ただし packages.tdm では `<<<` は1行のみ許可されます。

### 単一シンボル

```taida
<<< add
```

### ワンライナー

```taida
<<< double x: Int = x * 2 => :Int
```

### 構造化エクスポート

```taida
mathUtils <= @(
  add x: Int y: Int = x + y => :Int,
  subtract x: Int y: Int = x - y => :Int,
  PI <= 3.14159
)

<<< mathUtils
```

---

## モジュールの構成パターン

### ユーティリティモジュール（utils.td）

```taida
capitalize text: Str =
  | text == "" |> ""
  | _ |>
    first <= CharAt[text, 0]()
    rest <= Slice[text](start <= 1)
    Upper[first]() + rest
=> :Str

clamp value: Int min: Int max: Int =
  | value < min |> min
  | value > max |> max
  | _ |> value
=> :Int

<<< @(capitalize, clamp)
```

### 型定義モジュール（types.td）

```taida
Staff = @(
  id: Int
  name: Str
  rank: Str
  active: Bool
)

<<< @(Staff)
```

### メインモジュール（main.td）

```taida
>>> ./utils.td => @(capitalize, clamp)
>>> ./types.td => @(Staff)

asuka <= Staff(
  id <= 1,
  name <= "asuka",
  rank <= "Pilot",
  active <= true
)
displayName <= capitalize(asuka.name)
stdout(displayName)  // "Asuka"
```

---

## 実行モデル

### プログラムの実行

```bash
$ taida ./main.td
```

指定されたファイルのトップレベルコードが上から順に実行されます。

### ライブラリ vs 実行可能ファイル

ファイル自体に区別はありません。呼び出し方で決まります。

| 呼び出し方 | 動作 |
|------------|------|
| `$ taida ./file.td` | エントリーポイントとして実行されます |
| `>>> ./file.td => @(...)` | ライブラリとしてインポートされます |

### 二重実行の防止

同じファイルが複数回インポートされても、トップレベルコードは1回のみ実行されます。

### 循環参照の検出

循環参照は自動検出され、コンパイルエラーになります。

```taida
// a.td
>>> ./b.td => @(funcB)
<<< funcA

// b.td
>>> ./a.td => @(funcA)  // コンパイルエラー: 循環参照
<<< funcB
```

解決策は共通モジュールに抽出することです。

---

## packages.tdm（パッケージマニフェスト）

`packages.tdm` はパッケージの静的マニフェストです。依存宣言とパッケージの公開 API を1ファイルに集約します。

**packages.tdm は実行されません。** `>>>` と `<<<` のみが許可され、式・代入・関数定義は書けません。

```taida
// packages.tdm の例
>>> taida-lang/os@a.1
>>> ./main.td => @(hello, greet)

<<<@a.3 @(hello, greet)
```

### packages.tdm と通常の .td の違い

| | packages.tdm | 通常の .td |
|---|---|---|
| 実行 | されない（静的解釈のみ） | される |
| `>>>` のバージョン指定 | 可能（`@a.1` 等） | 不可 |
| `<<<` のバージョン指定 | 可能（`@a.3` 等、publish が管理） | 不可 |
| `<<<` の行数 | 1行のみ | 複数行可 |
| 式・代入・関数定義 | 禁止 | 可 |

### 役割の使い分け

| 用途 | エントリーポイント | packages.tdm の役割 |
|------|-------------------|-------------------|
| アプリ（`taida <file>`） | `main.td` | 依存宣言のみ |
| ライブラリ（`>>> author/pkg@a.1`） | packages.tdm が指す実コード | 依存 + 公開 API 宣言 |

---

## まとめ

| 構文 | 用途 |
|------|------|
| `stdout(value)` | 標準出力（プリリュード、import不要） |
| `>>> ./ファイル.td => @(シンボル)` | ローカルインポート |
| `>>> ./ファイル.td` | ローカルインポート（全エクスポートを取り込む） |
| `>>> npm:パッケージ => @(シンボル)` | npm パッケージインポート（JSのみ、Molten 型で取得） |
| `>>> パッケージ@ver => @(シンボル)` | バージョン指定インポート（packages.tdm のみ） |
| `<<< @(シンボル1, シンボル2)` | エクスポート（複数行可） |
| `<<< シンボル` | 単一エクスポート |

前のガイド: [関数](09_functions.md) | 次のガイド: [非同期処理](11_async.md)
