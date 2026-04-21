# Taida Lang -- 概要

> **PHILOSOPHY.md -- I.** 深く考えずに適当にぶちこんでけ

---

## Taida とは何か

Taida Lang は AI 協業時代のために設計されたプログラミング言語です。

演算子は10種のみに限定されており、null や undefined は存在しません。全ての型にデフォルト値が保証され、暗黙の型変換は一切行われません。操作はモールド（鋳型）で行い、メソッドは状態チェックと toString だけが残っています。

> **PHILOSOPHY.md -- IV.** キモい言語だ。だが、書くのも読むのもAI。眺めるのが人間の仕事。

AI が積極的に選定し、AI が書き、AI が読みます。人間は構造を眺めるだけで構いません。それが Taida の基本姿勢です。

---

## 技術的な特徴

Taida は3本の柱で成り立っています。

### 1. `><` -- ゴリラ哲学

`><` 怒りの表情がそのままリテラルになりました。プログラムにゴリラが出現する、すなわちコードが終了するということです。

```taida
| data.isEmpty() |> ><  // ゴリラが出現。プログラムは終了します。
```

他の言語では `sys.exit(1)` に11文字、`process.exit(1)` に15文字を要しますが、Taida では2文字で済みます。import も不要です。

また、カスタムエラーの throw をキャッチし忘れた場合は、暗黙のゴリラ天井がプログラムを停止させます。ゼロ除算や範囲外アクセスは Lax で安全に処理され、プログラムは止まりません。

詳しくは [エラー処理](08_error_handling.md) を参照してください。

### 2. 型のガチガチさ -- 厳格な型システム

Taida の型システムは柔軟さよりも厳格さを重視しています。

```taida
// これはエラーになります
"Count: " + 42              // コンパイルエラー。暗黙変換は行いません。

// 明示的に変換してください
"Count: " + Str[42]() ]=> _  // Str モールドで変換します
"Count: " + 42.toString()    // toString メソッドでも OK です
```

全ての型にデフォルト値があり、null は存在しません。外部データ（JSON）は溶鉄として扱われ、スキーマ（鋳型）を通さなければ Taida の型安全な世界に入れません。

詳しくは [型システム](01_types.md) と [型のガチガチさ](02_strict_typing.md) を参照してください。

### 3. 構造 -- ぶちパック、モールディング、グラフ

データは「ふくろ」にしまい、型が必要なら「鋳型」を作ります。そしてコードの構造はグラフとして抽出されます。

```taida
// ふくろ（ぶちパック）
pilot <= @(name <= "Rei", syncRate <= 78)

// 鋳型（モールディング型）
Mold[T] => Result[T, P <= :T => :Bool] = @(throw: Error)

// グラフ -- 10種の演算子だからこそ、決定的に抽出できます
data => Filter[_, _ x = x > 0]() => Map[_, _ x = x * 2]() => result
```

---

## なぜ AI は Taida を選ぶのか

### 固有演算子が10種しかない

覚えることが少ないというのは、AI にとっても同じです。生成するパターンが限定されるほど、正確なコードを書くことができます。

| 演算子 | 用途 |
|--------|------|
| `=` | 定義 |
| `=>` `<=` | ぶちこみ（データフロー） |
| `]=>` `<=[` | アンモールディング（値の取り出し） |
| `\|==` | エラー天井 |
| `\|` `\|>` | 条件分岐 |
| `>>>` `<<<` | モジュール（インポート/エクスポート） |

### グラフが決定的に抽出できる

演算子が少なく意味が一意であるため、構文を走査するだけでデータフローグラフを構築できます。推論も近似も不要です。AI が生成したコードの構造を、AI 自身が即座に検証できます。

### null がない

AI がコード生成する際に最も厄介なのは、「この変数は null かもしれない」という不確実性です。Taida では全ての型にデフォルト値が保証されているため、null チェックは不要です。

### 終端が明確

ゴリラリテラル `><` がある分岐は到達不能コードであり、`|==` がなければゴリラ天井が控えています。制御フローの終端が常に明確であるため、AI は安全なコードを生成しやすくなります。

---

## Hello World

```taida
greet name: Str =
  "Hello, " + name + "!"
=> :Str

message <= greet("World")
stdout(message)  // "Hello, World!"
```

`stdout` はプレリュードに含まれるビルトイン関数です。import は不要です。

---

## 基本的な構文

### 変数と代入

```taida
x <= 42
name <= "Asuka"
active <= true
```

### 型定義

```taida
Pilot = @(
  name: Str
  age: Int
  active: Bool
)

asuka <= Pilot(name <= "Asuka", age <= 14, active <= true)
```

### 関数定義

```taida
add x: Int y: Int =
  x + y
=> :Int

result <= add(3, 5)  // 8
```

### 条件分岐

```taida
// 複数行の多アーム条件は丸括弧で包みます（C20 / E0303）。
grade <= (
  | score >= 90 |> "A"
  | score >= 80 |> "B"
  | score >= 70 |> "C"
  | _ |> "D"
)
```

### パイプライン

```taida
// パイプライン: => で流す
data => Filter[_, _ x = x > 0]() => Map[_, _ x = x * 2]() => result
```

### エラー天井

```taida
|== error: Error =
  "Error: " + error.message
=> :Str

// この下でエラーが throw される可能性がある処理
riskyOperation()
```

### モジュール

```taida
>>> ./utils.td => @(helper, format)

result <= helper(data)

<<< @(myFunction, MyType)
```

---

## デフォルト値

全ての型にデフォルト値があります。null は存在しません。

| 型 | デフォルト値 |
|----|-------------|
| Int | `0` |
| Float | `0.0` |
| Str | `""` |
| Bool | `false` |
| @[T] (リスト) | `@[]` |
| @(...) (ぶちパック) | 各フィールドのデフォルト値 |
| JSON | `{}` (空オブジェクト) |
| Lax[T] | T のデフォルト値 |

```taida
// 型を定義すれば、省略されたフィールドはデフォルト値になります
NervStaff = @(name: Str, callSign: Str, age: Int)
ritchan <= NervStaff(name <= "Ritsuko")  // callSign = "", age = 0
```

---

## 他言語との比較

| 特徴 | Taida | TypeScript | Rust | Python |
|------|-------|------------|------|--------|
| 型安全性 | 厳格 | 中 | 極高 | 低 |
| null 安全 | 完全排除 | partial | Option | なし |
| メモリ管理 | 完全自動 | GC | 所有権 | GC |
| 学習コスト | 低 | 中 | 高 | 低 |
| AI 親和性 | 極高 | 中 | 低 | 中 |
| グラフ抽出 | 決定的 | 非決定的 | 非決定的 | 非決定的 |
| 操作の表現 | モールド | メソッド | メソッド | メソッド/関数 |
| 除算安全性 | Lax (常に値を返す) | Infinity | パニック (int) | ZeroDivisionError |

---

## ドキュメント構成

### ガイド

| # | ドキュメント | 内容 |
|---|------------|------|
| 00 | [概要](00_overview.md) | 本ドキュメント |
| 01 | [型システム](01_types.md) | プリミティブ型、Enum型、コレクション型、モールディング型、デフォルト値 |
| 02 | [型のガチガチさ](02_strict_typing.md) | 暗黙変換禁止、Lax による安全な操作、JSON エアロック |
| 03 | [JSON 溶鉄](03_json.md) | JSON の不透明プリミティブ化、スキーマ必須の型変換 |
| 04 | [ぶちパック](04_buchi_pack.md) | ぶちパック構文 |
| 05 | [モールディング](05_molding.md) | モールディング型（操作モールド全種） |
| 06 | [リスト操作](06_lists.md) | リスト操作（モールド + 状態チェックメソッド） |
| 07 | [制御フロー](07_control_flow.md) | 条件分岐、パターンマッチ |
| 08 | [エラー処理](08_error_handling.md) | Lax + throw/\|== + ゴリラ天井 |
| 09 | [関数](09_functions.md) | 関数定義、パイプライン、末尾再帰 |
| 10 | [モジュール](10_modules.md) | モジュールシステム（プリリュード、インポート/エクスポート） |
| 11 | [非同期処理](11_async.md) | Async[T]、]=> await |
| 12 | [イントロスペクション](12_introspection.md) | 構造的イントロスペクション |
| 13 | [アドオン作成](13_creating_addons.md) | Rust アドオンの作成とプレビルド配布 |

### リファレンス

| ドキュメント | 内容 |
|------------|------|
| [演算子リファレンス](../reference/operators.md) | 10種の演算子 + 算術・比較・論理 |
| [モールディング型リファレンス](../reference/mold_types.md) | 全モールドの型シグネチャ |
| [命名規則](../reference/naming_conventions.md) | 識別子の命名規則 |
| [CLI リファレンス](../reference/cli.md) | 実装準拠のコマンド一覧とオプション |
| [グラフモデル](../reference/graph_model.md) | 5つのグラフビュー |
| [ドキュメントコメント](../reference/documentation_comments.md) | AI 協業タグ |
| [末尾再帰最適化](../reference/tail_recursion.md) | TCO の判定ルール |
| [スコープルール](../reference/scope_rules.md) | スコープベース自動管理 |
| [アドオンマニフェスト](../reference/addon_manifest.md) | addon.toml の仕様 |
| [診断コード](../reference/diagnostic_codes.md) | コンパイラ診断コード一覧 |
| [標準ライブラリ](../reference/standard_library.md) | プリリュードとビルトイン型 |
| [標準メソッド](../reference/standard_methods.md) | 状態チェック・モナディックメソッド |

### 設計ドキュメント

| ドキュメント | 内容 |
|------------|------|
| [メソッド→モールド リファクタリング](../design/method_to_mold_refactoring.md) | 操作モールド設計 |
| [JSON 溶鉄化](../design/json_molten_iron.md) | JSON の不透明プリミティブ化設計 |
| [CLI ドキュメント運用設計](../design/cli_documentation.md) | CLI 文書の同期ルールと更新規約 |
| [Graph/Human 収益化設計](../design/graph_human_monetization.md) | 無料 `taida` と有料 `taida-human` の責務分離 |
