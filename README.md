# Taida Lang

Taida Lang は AI 協業時代に向けて設計されたプログラミング言語です。

10 種類の演算子しか持たず、`null` や `undefined` は存在せず、暗黙の
型変換も許しません。「AI が書き、AI が読み、人間は構造を眺める」を
基本姿勢とし、コードをグラフとして解析しやすい形に整えてあります。

## 特徴

- 演算子は 10 種類のみ (`=`, `=>`, `<=`, `]=>`, `<=[`, `|==`, `|`, `|>`, `>>>`, `<<<`)
- すべての型にデフォルト値があり、null / undefined は存在しない
- 構造化データはぶちパック `@(...)` で表現
- パラメトリック型はモールド `Mold[T]` で表現
- Enum 型は序数値を持つ
- 3 バックエンド (インタプリタ / JS / ネイティブ) + WASM プロファイル
  群が同一出力を返すパリティ運用
- `taida way` / `taida graph` / `taida doc` による構造的ツーリング
- Rust 製ネイティブアドオン (`cdylib`) による言語拡張

## Hello World

```taida
greet name: Str =
  "Hello, " + name + "!"
=> :Str

message <= greet("World")
stdout(message)
```

`stdout` はプレリュード経由で常に利用可能なため、インポートは不要です。

## 例: Enum とぶちパック

```taida
Enum => Status = :Active :Inactive :Pending

Pilot = @(
  name: Str
  sync_rate: Int
)

pilot <= Pilot(name <= "Rei", sync_rate <= 78)
stdout(pilot.name)
stdout(Status:Active() == Status:Active())   // true
```

## バックエンド

| バックエンド | コマンド | 出力 |
|--------------|----------|------|
| インタプリタ | `taida file.td` | 直接実行 |
| ネイティブ | `taida build file.td` | バイナリ実行ファイル (既定) |
| JS | `taida build js file.td` | `.mjs` ファイル |
| WASM | `taida build wasm-wasi file.td` | `.wasm` ファイル |

すべてのバックエンドは、同一ソースから同一出力を返します
(3-way パリティ運用)。WASM プロファイルは `wasm-min` / `wasm-wasi` /
`wasm-edge` / `wasm-full` の 4 種で、各プロファイルの対応範囲は
[WASM プロファイル](docs/reference/wasm_profiles.md) を参照してください。

メモリ管理は完全自動です。バックエンドごとの具体的な戦略は
[メモリ管理モデル](docs/reference/memory_model.md) に整理しています。

## インストール

公開配布チャネルは `taida.dev` の `install.sh` です。`crates.io` は
リリースチャネルとしては利用しません。

タグ付けされたリリースは Linux / macOS / Windows 向け GitHub Release
アーティファクトとして配布され、共通の `SHA256SUMS` ファイルが付属
します。

ソースからビルドする場合は次のとおりです。

```bash
cargo build --release
./target/release/taida examples/01_hello.td
```

テスト実行:

```bash
cargo test
```

## コア同梱パッケージ

Taida バイナリには次のコアパッケージが同梱されており、`taida ingot install`
は不要です。`>>> taida-lang/<pkg>` で明示インポート、または import 不要
での直接呼び出しの両方をサポートします。

| パッケージ | 説明 |
|------------|------|
| `taida-lang/os` | ファイル / プロセス / 環境変数 / 低水準ソケット / DNS |
| `taida-lang/net` | HTTP サーバー / クライアント、WebSocket、SSE |
| `taida-lang/crypto` | 暗号系プリミティブ (`sha256`) |
| `taida-lang/js` | JS バックエンド向け相互運用 (モールテン境界) |
| `taida-lang/pool` | 接続プーリング (`poolCreate` / `poolAcquire` 他) |

各パッケージのインポート形式・公開シンボル・バックエンド対応は
[API リファレンス](docs/api/README.md) を参照してください。

`taida-lang/terminal` のような公式アドオンは、コア同梱とは別カテゴリです。
`taida ingot install` で取得し、`packages.tdm` で依存宣言します。
詳細は [アドオン作成ガイド](docs/guide/13_creating_addons.md) を参照
してください。

## アドオン

Rust 製ネイティブアドオン (`cdylib` クレート) を介して言語機能を拡張
できます。

```bash
taida init --target rust-addon my-addon   # 雛形生成 (release.yml 含む)
taida ingot publish --dry-run             # 次のバージョンをプレビュー
taida ingot publish                       # タグ push、CI がリリース作成
taida ingot install                       # プレビルドのダウンロード
```

`taida ingot publish` は git タグを push して即終了します。アドオン
リポジトリ側の CI (`taida init` が雛形生成する
`.github/workflows/release.yml`) が 5 プラットフォーム cdylib マトリクス
をビルドし、`github-actions[bot]` として GitHub Release を作成します。
詳細と移行手順は [アドオン作成ガイド](docs/guide/13_creating_addons.md)
を参照してください。

## ドキュメント

### ガイド

| # | ドキュメント | 内容 |
|---|------------|------|
| 00 | [概要](docs/guide/00_overview.md) | 言語概要 |
| 01 | [型システム](docs/guide/01_types.md) | プリミティブ、Enum、コレクション、モールド型 |
| 02 | [型のガチガチさ](docs/guide/02_strict_typing.md) | 暗黙変換禁止、Lax 安全操作、JSON のスキーマ必須 |
| 03 | [JSON 溶鉄](docs/guide/03_json.md) | JSON の不透明プリミティブ化とスキーマ必須キャスト |
| 04 | [ぶちパック](docs/guide/04_buchi_pack.md) | ぶちパック構文 |
| 04+ | [クラスライク型定義](docs/guide/04_class_like.md) | 統一構文 (構造化データ型 / モールド / エラー) |
| 05 | [モールド](docs/guide/05_mold.md) | モールド型の解剖、`solidify` / `unmold`、ユーザー定義 |
| 06 | [リスト操作](docs/guide/06_lists.md) | リストモールドと状態チェックメソッド |
| 07 | [制御フロー](docs/guide/07_control_flow.md) | 条件分岐とパターンマッチ |
| 08 | [エラー処理](docs/guide/08_error_handling.md) | Lax + throw/\|== + ゴリラ天井 |
| 09 | [関数](docs/guide/09_functions.md) | 関数定義、パイプライン、末尾再帰、defaultFn |
| 10 | [モジュール](docs/guide/10_modules.md) | インポート / エクスポート、プレリュード |
| 11 | [非同期処理](docs/guide/11_async.md) | `Async[T]` と `]=>` await |
| 12 | [イントロスペクション](docs/guide/12_introspection.md) | 構造的内省 |
| 13 | [アドオン作成](docs/guide/13_creating_addons.md) | Rust アドオン作成と配布 |

コア同梱パッケージの API リファレンスは [`docs/api/`](docs/api/) にあります。

### リファレンス

| ドキュメント | 内容 |
|--------------|------|
| [CLI](docs/reference/cli.md) | `taida` CLI コマンドとフラグ |
| [演算子](docs/reference/operators.md) | 演算子と算術 / 比較 / 論理演算 |
| [命名規則](docs/reference/naming_conventions.md) | 識別子の命名規則とバージョン記法 |
| [グラフモデル](docs/reference/graph_model.md) | 5 つのグラフビュー |
| [ドキュメントコメント](docs/reference/documentation_comments.md) | AI 協業タグ |
| [末尾再帰](docs/reference/tail_recursion.md) | TCO の判定ルール |
| [スコープルール](docs/reference/scope_rules.md) | スコープベース自動管理 |
| [プレリュード関数 / ビルトイン型メソッド](docs/api/prelude.md) | プレリュード API、ビルトイン型のメソッド、HashMap / Set |
| [アドオンマニフェスト](docs/reference/addon_manifest.md) | `addon.toml` のスキーマと前方互換ポリシー |
| [メモリ管理モデル](docs/reference/memory_model.md) | 4 バックエンドのメモリ戦略とアドオン所有権規約 |
| [ビルド記述子](docs/reference/build_descriptors.md) | 複数ターゲットを束ねるビルド構成 |
| [パフォーマンスゲート](docs/reference/perf_gates.md) | スループット / RSS / Valgrind / カバレッジゲート |
| [WASM プロファイル](docs/reference/wasm_profiles.md) | WASM ターゲットプロファイルと対応範囲 |
| [リリースプロセス](docs/reference/release_process.md) | 世代番号・ビルド番号・互換性判断 |
| [診断コード](docs/reference/diagnostic_codes.md) | コンパイラ診断コード一覧 |

### API リファレンス

| ドキュメント | 内容 |
|--------------|------|
| [API リファレンス索引](docs/api/README.md) | `docs/api/` 全体の入口 |
| [プレリュード関数](docs/api/prelude.md) | `stdout` / `stdin` / `nowMs` / `sleep` / `jsonEncode` / `debug` / `typeof` / `range` / `exit` 等 |
| [`taida-lang/os`](docs/api/os.md) | ファイル I/O・プロセス・環境・ソケット・DNS の API |
| [`taida-lang/net`](docs/api/net.md) | HTTP/1.1・H2・H3・WebSocket・SSE の API |
| [`taida-lang/crypto`](docs/api/crypto.md) | `sha256` の API |
| [`taida-lang/js`](docs/api/js.md) | JS 相互運用 descriptor の API |
| [`taida-lang/pool`](docs/api/pool.md) | リソースプーリングの API |

## バージョニング

公開リリースの正本識別子は Taida バージョンであり、Rust パッケージ
の semver ではありません。

- Taida バージョンは `@<世代>.<番号>[.<ラベル>]` の形式で記述します。
  詳細は [リリースプロセス](docs/reference/release_process.md) と
  [命名規則](docs/reference/naming_conventions.md) を参照してください。
- `Cargo.toml` の semver は Rust ツーリングのためのメタデータです。

リリースノートと公開コミュニケーションでは Taida の
`@<世代>.<番号>[.<ラベル>]` を主に用い、Cargo の semver は補助情報として
扱います。

## ライセンス

`Cargo.toml` で MIT ライセンスを宣言しています。詳細は同ファイルおよび
リポジトリのライセンスファイルを参照してください。
