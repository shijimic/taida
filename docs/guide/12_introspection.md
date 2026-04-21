# 構造的イントロスペクション

> **PHILOSOPHY.md — IV.** キモい言語だ。だが、書くのも読むのもAI。眺めるのが人間の仕事。
>
> 系: 「コードの構造は、コード自身が語る」

---

## 構造的イントロスペクションとは

Taida は10種の演算子で全てを表現します。各演算子の意味は一意で、重複しません。そのため、**構文を走査するだけでコードの構造をグラフとして構築できます**。型推論や制御フロー解析は不要です。

他の言語では同じ処理を複数の構文で書けます（`return`, `yield`, `.then()`, コールバックなど）。Taida ではデータフローは `=>` か `<=` のどちらかだけです。この制約により、グラフが一意に確定します。

| 演算子 | グラフ上の意味 |
|--------|---------------|
| `=` | ノード生成 |
| `=>` | 順方向エッジ |
| `<=` | 逆方向エッジ |
| `]=>` | アンモールドエッジ（順方向） |
| `<=[` | アンモールドエッジ（逆方向） |
| `\|==` | エラー境界ノード |
| `\|` `\|>` | 条件エッジ |
| `>>>` | 依存エッジ（入） |
| `<<<` | 依存エッジ（出） |

---

## 5つのグラフビュー

### 1. データフローグラフ

`=>` / `<=` / `]=>` / `<=[` を追跡して、値がどこからどこへ流れるかを可視化します。

```taida
input <= "  asuka langley  "
input => Trim[_]() => Upper[_]() => result
```

```
[input: "  asuka langley  "] --(PipeForward)--> [Trim[_]()]
  --(PipeForward)--> [Upper[_]()]
  --(PipeForward)--> [result]
```

### 2. モジュール依存グラフ

`>>>` / `<<<` を追跡して、ファイル間の依存関係を可視化します。

```taida
>>> ./utils.td => @(helper, format)
>>> ./types.td => @(Staff, Config)
<<< @(main)
```

```
[main.td] --(Imports)--> [./utils.td] {helper, format}
[main.td] --(Imports)--> [./types.td] {Staff, Config}
[main.td] --(Exports)--> {main}
```

### 3. 型階層グラフ

`Mold[T] =>` や `Error =>` による継承関係を追跡します。

```
Staff
  +-- Commander
  +-- Operator

Mold[T]
  +-- Result[T, P]
  +-- Lax[T]
  +-- Async[T]

Error
  +-- ValidationError
  +-- NetworkError
```

### 4. エラー境界グラフ

`|==` と `.throw()` サイトの関係を追跡します。

```
[processData]
  +-- ErrorCeiling(Error)
        +-- Catches <-- ThrowSite(InputError) at line 8
        +-- Catches <-- (any Error thrown by transform())
```

ゴリラ天井は暗黙のエラー境界として、キャッチされなかったエラーを最終的に受け止めます。

### 5. コールグラフ

関数呼び出しの関係を追跡します。

```
processAll
  +-- Calls --> double
  |               +-- Calls --> add
  +-- Calls --> Map[...]
  +-- Calls --> Filter[...]
  +-- Calls --> add
```

---

## verify（構造的自己検証）

`taida verify` コマンドで、コードの構造的な健全性を検証できます。  
最新のCLIオプションは [CLI リファレンス](../reference/cli.md) を参照してください。

### CLI コマンド

```bash
# 全検証を実行します
taida verify ./src

# 特定の検証のみ
taida verify --check direction-constraint ./src
taida verify --check error-coverage ./src
taida verify --check no-circular-deps ./src
taida verify --check dead-code ./src

# JSON / SARIF 出力
taida verify --format json ./src
taida verify --format sarif ./src
```

### 出力例

```
$ taida verify ./src
[PASS] direction-constraint: 全ファイルが単一方向制約を満たしています
[PASS] error-coverage: 全ての throw サイトがエラー天井でカバーされています
[WARN] dead-code: 2つの未使用関数が検出されました
  - src/utils.td:15 deprecatedHelper
  - src/utils.td:42 oldFormat
[PASS] no-circular-deps: 循環依存はありません
[PASS] type-consistency: 型階層に不整合はありません

結果: 4 passed, 1 warning, 0 errors
```

### グラフの抽出・可視化

```bash
# データフローグラフ
taida graph --type dataflow ./src/main.td

# モジュール依存グラフ
taida graph --type module ./src/main.td

# 型階層グラフ
taida graph --type type-hierarchy ./src/main.td

# エラー境界グラフ
taida graph --type error ./src/main.td

# コールグラフ
taida graph --type call ./src/main.td

# 出力形式の指定
taida graph --type dataflow --format mermaid ./src/main.td
taida graph --type module --format dot ./src/main.td
taida graph --type call --format json ./src/main.td

# 構造サマリの生成
taida graph summary ./src/main.td
```

### 構造クエリ

グラフに対して構造的なクエリを発行できます。

```bash
# パス存在性
taida graph query --type dataflow --query "path_exists(input, result)" ./src/main.td
# => true: input -> Trim -> Upper -> result

# 循環検出
taida graph query --type module --query "find_cycles()" ./src/main.td
# => No cycles found.

# 到達可能性
taida graph query --type dataflow --query "reachable(input)" ./src/main.td
# => [Trim, Upper, result]

# エラーカバレッジ
taida graph query --type error --query "uncovered_throws()" ./src/main.td
# => All throw sites are covered.

# デッドコード検出
taida graph query --type call --query "unreachable_functions()" ./src/main.td
# => [unusedHelper, deprecatedFunc]
```

---

## AI 協業タグ

ドキュメントコメントにタグをつけることで、AI との協業を促進します。

### `@AI-Related`

関連するシンボルを明示します。AI がコードを変更する際に、影響範囲を把握するために使います。

```taida
///@ AI-Related: validateStaff, saveStaff, Staff
createStaff name: Str rank: Str =
  validated <= validateStaff(name, rank)
  saveStaff(validated)
=> :Staff
```

`@AI-Related` タグは現状 `taida verify` の個別チェック対象ではありません。  
`taida doc generate` で抽出されるドキュメント情報として活用します。

### `@AI-SideEffects`

副作用を持つ関数であることを明示します。

```taida
///@ AI-SideEffects: writes to database
saveRecord record: Record =
  ...
=> :Result[Record, _]
```

### `@Throws`

関数がスローする可能性のあるエラー型を明示します。

```taida
///@ Throws:
///@   - ValidationError: 入力が不正な場合
///@   - NetworkError: 接続エラー
fetchStaff id: Int =
  ...
=> :Staff
```

`@Throws` タグも同様に、現状はドキュメント用途で扱います。

---

## 構造サマリ

AI が消費するための機械可読フォーマットです。

```bash
taida graph summary ./src/main.td
```

```json
{
  "version": "1.0",
  "project": "nerv-ops",
  "stats": {
    "files": 12,
    "functions": 45,
    "types": 18,
    "mold_types": 6,
    "error_types": 4
  },
  "dataflow": {
    "total_pipes": 128,
    "forward_pipes": 95,
    "backward_pipes": 33,
    "unmold_operations": 22
  },
  "modules": {
    "total_imports": 34,
    "total_exports": 28,
    "has_cycles": false
  },
  "errors": {
    "total_ceilings": 15,
    "total_throw_sites": 23,
    "uncovered_throws": 0
  }
}
```

---

## ランタイム観測 — `debug()` の flush タイミング

構造的イントロスペクションは**実行前**にコードを眺めるための道具ですが、パイプラインの途中に `debug()` を差し込むと**実行中**に値を観察できます。

```taida
scores <= @[95, 82, 78, 91]
scores => Filter[_, _ x = x > 80]() => debug => Map[_, _ x = x * 2]() => result
```

`debug(value)` は渡された値をそのまま返すため、パイプラインを切断せずに観測地点を挟めます。単独で値を print したいときや、ラベル付きで `[label] Type: repr` 形式を出したいときは以下を使います。

```taida
debug("ping")                 // "ping" を出力
debug(user, "load_user")      // "[load_user] BuchiPack: @(name: ..., age: ...)" を出力
```

### flush タイミング（C22 以降）

| 実行モード | `stdout(...)` | `debug(...)` | 備考 |
|---|---|---|---|
| CLI (`taida <file>` / `taida run <file>`) | 呼び出し毎に即 flush | 呼び出し毎に即 flush | 長時間実行のスクリプトで進捗・トレースをリアルタイム観測できる |
| REPL (`taida repl`) | eval 完了後に一括出力 | eval 完了後に一括出力 | return-value 字下げ表示との視覚分離のためバッファモードを維持 |
| Rust 側 in-process test | eval 完了後に `interpreter.output` から取得 | 同左 | `Interpreter::new()` 経由のテスト互換性を維持 |

`debug` の stream mode 出力先は **stdout** です（stderr ではありません）。これは JS バックエンド（`console.log`）・Native バックエンド（`printf`）との 3 バックエンドパリティを保証するための設計判断で、`test_native_compile_parity` 系の captured-stdout diff を破らない形に揃えています。

長時間ループの中で `debug(x)` を差し込めば、プログラムが止まらずとも途中経過がそのまま端末に流れてくるため、printf-デバッグが機能します。これは post-C20 で「`stdout` / `debug` だけが Vec にバッファされて終了まで 1 バイトも見えない」問題の対称性回復として修正されました。Taida の surface API 仕様は変わっていないため、既存コードの書き換えは不要です。

---

## 他言語との比較

| 特性 | Taida | TypeScript | Rust | Python |
|------|-------|------------|------|--------|
| 演算子数 | 10 | 50+ | 60+ | 30+ |
| データフロー方向の確定性 | 決定的 | 非決定的 | 非決定的 | 非決定的 |
| グラフ抽出に必要な解析 | 構文走査のみ | 型推論 + 制御フロー解析 | 所有権解析 | 動的解析が必要 |
| 単一方向制約 | あり | なし | なし | なし |

Taida の強みは、構文走査だけでグラフが構築でき、AI が生成したコードの構造を即座に検証できる点にあります。

---

## まとめ

| 概念 | 内容 |
|------|------|
| グラフ抽出 | 演算子の意味が一意なので構文走査だけで構築できます |
| 5つのビュー | DataFlow, Module, TypeHierarchy, Error, CallGraph |
| verify | 構造的な健全性を自動検証します |
| AI 協業タグ | `@AI-Related`, `@AI-SideEffects`, `@Throws` |
| 構造サマリ | AI が消費する JSON フォーマットで出力できます |

前のガイド: [非同期処理](11_async.md)
