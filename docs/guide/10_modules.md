# モジュールシステム

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ

モジュール管理は2つの演算子だけです。`>>>` がインポート、`<<<` がエクスポートです。よく使う関数はプレリュードとして最初から使えます。

---

## プレリュード

インポート不要で、どのファイルからでも使えるモールドと関数の体系です。完全な一覧と仕様は [プレリュード API](../api/prelude.md) を参照してください。

```taida
// import は不要です
stdout("Hello, World!")

pilot <= @(name <= "Rei", age <= 14)
json <= jsonEncode(pilot)
stdout(json)  // {"name":"Rei","age":14}

// 型名を取り出したい場合はモールド TypeName を使います
TypeName[42]()  // "Int"
```

モールド体系全体は [モールド](05_mold.md) を参照してください。

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

### npm パッケージ（旧 JS ターゲットのみ）

`>>> npm:package => @(symbols)` で npm パッケージをインポートします。旧 JS ターゲットでのみ動作します。

npm からインポートされた値は **Molten** 型（branch=JS）です。Molten は外部由来の不透明値であり、直接操作はできません。型安全な値を取り出すには `Cage[subject, JSRilla[...]()]()` 経由で操作します。

```taida fragment
>>> npm:express => @(express)  // express: Molten (branch=JS)

// express() を呼び出してアプリ handle を得る
Cage[express, JSCall[@[], @[], Molten]()]() >=> app       // app: Molten

// new express.Router() でルータを生成
Cage[express, JSNew[@["Router"], @[], Molten]()]() >=> router  // router: Molten

// app.listen(3000) を Int として受け取る
Cage[app, JSCall[@["listen"], @[3000], Int]()]() => result    // result: Gorillax[Int]
result >=> server                                              // server: Int（またはゴリラ）

>>> npm:node:timers/promises => @(setTimeout)

// Promise を返す関数は Async として待つ
Cage[setTimeout, JSCallAsync[@[], @[20, true], Bool]()]() >=> ready  // ready: Bool
```

**型の流れ:**

```
npm import (Molten / branch=JS)
  -> JSRilla 実行記述 (JSGet / JSCall / JSCallAsync / JSNew / JSSet / JSBind / JSSpread)
  -> Cage (subject branch ↔ runner branch を照合)
  -> Gorillax[Out] または Async[Out]
  -> >=> 値
```

インタプリタおよび Native バックエンドで `npm:` インポートを使用するとコンパイルエラーになります。`JSRilla` 子系統 (`JSGet` / `JSCall` / `JSCallAsync` / `JSNew` / `JSSet` / `JSBind` / `JSSpread`) も同様です。

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

### 定義してからエクスポート

エクスポート文に関数定義を直接書くことはできません。定義と `<<<` は別の文に分けます。

```taida fragment
double x: Int = x * 2 => :Int
<<< double
```

### 構造化エクスポート

```taida
mathUtils <= @(
  add <= _ x: Int y: Int = x + y,
  subtract <= _ x: Int y: Int = x - y,
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

> `| _ |>` の本体は単一式でも、中間バインド (`name <= expr` など) を並べた後に末尾式または末尾バインドで締める複数文ブロックでも書けます。詳しくは [制御フロー](07_control_flow.md) を参照してください。

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

**packages.tdm は実行されません。** `>>>` / `<<<` と、source package の SHA-256 pin を持つ `[packages."owner/name"]` テーブルのみが許可され、式・代入・関数定義は書けません。

### 公開 API の解決規則

`<<<@a.X owner/pkg @(symbol1, symbol2)` で公開するシンボルは、**`>>> ./main.td` で指定したエントリーモジュールが `<<<` でエクスポートしたシンボルからのみ解決されます**。他の依存パッケージ (例: `taida-lang/os`) で同名のシンボルが存在しても、公開対象には含まれません。

```taida fragment
// ./main.td
>>> taida-lang/os@a.1 => @(stdout)

hello name: Str =
  "Hello, " + name + "!"
=> :Str

greet =
  stdout(hello("World"))
=> :Int

<<< @(hello, greet)
```

```taida fragment
// packages.tdm
>>> taida-lang/os@a.1
>>> ./main.td

<<<@a.3 owner/pkg @(hello, greet)
```

この例で公開される `hello` と `greet` は、必ず `./main.td` 内で定義され、`./main.td` の `<<<` でエクスポートされている必要があります。`./main.td` 側で `<<<` が抜けていたり、定義されていないシンボル名が `<<<@a.3` に書かれていた場合は、consumer 側の `>>> owner/pkg@a.3 => @(...)` 解決時にエラーになります。

### source tarball 依存

source tarball から取得する依存は `[packages."taida-lang/name"]` テーブルで宣言し、SHA-256 pin を必ず併記します。現時点で source package の owner として受理されるのは `taida-lang` のみです。`integrity` は `sha256:` + 64 文字の小文字 hex です。download した tarball の SHA-256 が一致しない場合、install は `[E32K3_SOURCE_INTEGRITY_MISMATCH]` で中断します。

```toml
[packages."taida-lang/web"]
version = "a.1"
integrity = "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
```

source tarball は `https://github.com/taida-lang/...` からのみ取得され、`TAIDA_GITHUB_BASE_URL` による本番 override は受理されません。通常の source package install は manifest の SHA-256 pin を trust root にします。`TAIDA_VERIFY_SIGNATURES=required` を明示した場合は source archive の cosign bundle も必須になりますが、GitHub の自動 tag archive には release asset と違って同居する bundle がないため、その場合は `[E32K3_SOURCE_COSIGN_REQUIRED]` で停止します。

### packages.tdm と通常の .td の違い

| | packages.tdm | 通常の .td |
|---|---|---|
| 実行 | されない（静的解釈のみ） | される |
| `>>>` のバージョン指定 | 可能（`@a.1` 等） | 不可 |
| `<<<` のバージョン指定 | 可能（`@a.3` 等、publish が管理） | 不可 |
| `[packages."owner/name"]` | source tarball の SHA-256 pin | 不可 |
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
| `stdout(value)` | 標準出力（プレリュード、import不要） |
| `>>> ./ファイル.td => @(シンボル)` | ローカルインポート |
| `>>> ./ファイル.td` | ローカルインポート（全エクスポートを取り込む） |
| `>>> npm:パッケージ => @(シンボル)` | npm パッケージインポート（JSのみ、Molten 型で取得） |
| `>>> パッケージ@ver => @(シンボル)` | バージョン指定インポート（packages.tdm のみ） |
| `<<< @(シンボル1, シンボル2)` | エクスポート（複数行可） |
| `<<< シンボル` | 単一エクスポート |

前のガイド: [関数](09_functions.md) | 次のガイド: [非同期処理](11_async.md)
