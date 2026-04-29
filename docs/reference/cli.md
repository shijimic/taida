# Taida CLI リファレンス

> 実装正本: `src/main.rs`

このページは `@e.X` 以降の Taida CLI の単一リファレンスです。E31 は予告期間なしの破壊的変更として、旧トップレベルコマンドを `taida way` / `taida ingot` 配下へ整理します。タグごとの land 履歴は `CHANGELOG.md` を参照してください。

---

## 実行モード

### REPL

```bash
taida
taida --no-check
```

- 引数なしで REPL を起動します。
- `--no-check` を付けると REPL 内の型チェックをスキップします。

### ファイル実行

```bash
taida <FILE>
taida --no-check <FILE>
```

- サブコマンド以外の第1引数はファイルパスとして扱われます。
- 実行前のゲートは parse + type + `error-coverage` の警告のみで、軽量経路です。
- `--no-check` で型チェックをスキップできます。

### 共通フラグ

- `--help` / `-h`: トップレベルまたは各コマンドのヘルプを表示します。
- `--version` / `-V`: バージョンを表示します。
- `--no-check`: `taida`, `taida <FILE>`, `taida build` でのみ意味を持ちます。`taida way` 配下では拒否されます。

---

## 公開トップレベルコマンド

| コマンド | 用途 |
|---|---|
| `taida` / `taida <FILE>` | REPL / インタプリタ実行 |
| `build` | Native / JS / WASM 成果物の生成 |
| `way` | 品質ハブ。check / lint / verify / todo を集約 |
| `graph` | 構造グラフの抽出 / summary / query |
| `doc` | ドキュメントコメントから Markdown を生成 |
| `ingot` | パッケージ・依存ハブ。deps / install / update / publish / cache を集約 |
| `init` | プロジェクト雛形の生成 |
| `lsp` | LSP サーバーの起動 |
| `auth` | 認証状態の管理 |
| `community` | コミュニティ API へのアクセス |
| `upgrade` | Taida 本体のセルフアップデート |

`check` / `verify` / `lint` / `todo` / `inspect` / `transpile` / `compile` / `deps` / `install` / `update` / `publish` / `cache` / `c` は `@e.X` で削除されました。旧コマンドを叩いた場合は `[E1700]` と移行先を表示し、終了コード `2` で終了します。

| `@e.X` で削除 | 移行先 |
|---|---|
| `taida check` | `taida way check`、または完全実行の `taida way` |
| `taida verify` | `taida way verify` |
| `taida lint` | `taida way lint` |
| `taida todo` | `taida way todo` |
| `taida inspect` | `taida graph summary` |
| `taida transpile` | `taida build js` |
| `taida compile` | `taida build native` |
| `taida deps` | `taida ingot deps` |
| `taida install` | `taida ingot install` |
| `taida update` | `taida ingot update` |
| `taida publish` | `taida ingot publish` |
| `taida cache` | `taida ingot cache` |
| `taida c` | `taida community` |

---

## `taida build`

```bash
taida build [native|js|wasm-min|wasm-wasi|wasm-edge|wasm-full] [--release] [--diag-format text|jsonl] [-o OUTPUT] [--entry ENTRY] <PATH>
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--output <PATH>` / `--outdir <DIR>` | `-o` | 出力先 |
| `--entry <PATH>` | - | Native + dir入力時のエントリ上書き |
| `--release` | `-r` | TODO/Stub 残存時に失敗 |
| `--diag-format <text\|jsonl>` | - | 診断出力形式 |

挙動:
- ターゲットは位置引数で指定します。省略時は `native` です。
- `taida build native <PATH>` は Native バイナリを生成します。
- `taida build js <PATH>` は `.mjs` を生成します。
- `taida build wasm-* <PATH>` は `.wasm` を生成します。
- 旧 `--target <target>` / `--target=<target>` フラグは `@e.X` で削除され、`[E1700]` + exit 2 になります。
- 既定では parse + type check を実行します。`--no-check` で型検査をスキップできます。
- `--diag-format jsonl` を指定するとコンパイル診断を `taida.diagnostic.v1` JSONL 形式で出力します。

WASM SIMD (`simd128`) 有効化ポリシー:

| プロファイル | `-msimd128` | 生成 wasm が要求する feature | 推奨ランタイム |
|---|---|---|---|
| `wasm-min` | なし | simd128 を要求しない | simd128 非対応ランタイムでも実行可 |
| `wasm-wasi` | あり | `simd128` を要求 | wasmtime 42+ |
| `wasm-edge` | あり | `simd128` を要求 | Cloudflare Workers などのエッジランタイム |
| `wasm-full` | あり | `simd128` を要求 | wasmtime 42+ |

---

## `taida way`

```bash
taida way <PATH>
taida way check [--format text|json|jsonl|sarif] [--strict] [--quiet] <PATH>
taida way lint [--format text|json|jsonl|sarif] [--strict] [--quiet] <PATH>
taida way verify [--check CHECK] [--format text|json|jsonl|sarif] [--strict] [--quiet] <PATH>
taida way todo [--format text|json|jsonl|sarif] [--strict] [--quiet] [PATH]
```

`taida way <PATH>` は品質ハブをまとめて実行する全工程ゲートです。`taida way` のように PATH を省略した場合は hub help を表示します。full gate は以下を直列に実行し、ERROR の検知が1件でもあれば終了コード `1` で終了します。

1. `taida way check <PATH>`: parse + type
2. `taida way lint <PATH>`: 命名規則 (`E1801`〜`E1809`)
3. `taida way verify <PATH>`: 構造検証

共通オプション:

| オプション | 説明 |
|---|---|
| `--format <text\|json\|jsonl\|sarif>` | 出力形式 |
| `--strict` | WARNING も終了コード `1` として扱う |
| `--quiet` / `-q` | 診断出力を抑制し、終了コードだけを返す |

`--no-check` は `taida way` 配下では拒否されます。品質ハブの責務と矛盾するためです。

### `way verify` のチェック一覧

| チェック | 重要度 | 概要 |
|---|---|---|
| `error-coverage` | error | throw サイトのカバレッジ検証 |
| `no-circular-deps` | error | モジュールの循環依存検出 |
| `dead-code` | warning | 到達不能関数の検出 |
| `type-consistency` | error | 型階層の循環検出 |
| `mutual-recursion` | error | 相互再帰構造の検証 |
| `unchecked-division` | warning | 除算の安全性チェック |
| `direction-constraint` | error | 単一方向制約 |
| `unchecked-lax` | warning | Lax の未検査利用検出 |

`naming-convention` は `way lint` の責務です。`way verify` では実行しません。

---

## `taida graph`

```bash
taida graph [-o OUTPUT] [--recursive] <PATH>
taida graph summary [--format text|json|sarif] <PATH>
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--recursive` | `-r` | import を再帰的にたどり、複数ファイルを統合した JSON を出力 |
| `--output <OUTPUT>` | `-o` | graph JSON の出力先。裸のファイル名は `.taida/graph/` 配下に保存 |
| `--format <text|json|sarif>` | `-f` | `graph summary` 用。構造サマリの出力形式 |

`graph summary` は旧 `inspect` の構造サマリ部分だけを提供します。verify の結果は含めません。必要に応じて `taida way verify --format sarif` と併用してください。

---

## `taida init`

```bash
taida init [--target rust-addon] [DIR]
```

| オプション | 説明 |
|---|---|
| `--target rust-addon` | Rust アドオン入り ingot の雛形を生成 |

挙動:
- `DIR` を省略した場合はカレントディレクトリに初期化します。
- 通常モードでは `packages.tdm`、`main.td`（存在しない場合のみ）、`.taida/` を作成します。
- `--target rust-addon` を指定すると `Cargo.toml`、`src/lib.rs`、`native/addon.toml`、`taida/<name>.td`、`README.md` を含むプロジェクトを作成します。

---

## `taida ingot`

```bash
taida ingot [--help]
taida ingot install [--force-refresh | --no-remote-check] [--allow-local-addon-build]
taida ingot deps
taida ingot update [--allow-local-addon-build]
taida ingot publish [--label LABEL] [--force-version VERSION] [--retag] [--dry-run]
taida ingot cache [clean] [--addons|--store|--store-pkg <org>/<name>|--all] [--yes]
```

`ingot` は Taida のパッケージ・依存ハブです。Rust の `crate` に相当する配布単位を Taida では ingot と呼びます。`addon` は ingot 内に含まれうるネイティブローダー接続単位で、`addon.toml` の名前は変更しません。

`taida ingot` はサブコマンド専用ハブです。引数なしで実行した場合はヘルプだけを表示し、インストールは行いません。`taida ingot <author/package>` のような形式はありません。依存は `packages.tdm` の `>>> author/pkg@a.1` で宣言してください。

### `ingot deps`

旧 `taida deps` の挙動をそのまま `ingot` 配下へ移したものです。

- `packages.tdm` をカレントから親方向に向かって探索します。
- 依存解決エラーが1件でもある場合、インストールと lockfile 更新を行わずに失敗します。
- 成功時は `.taida/deps` と `.taida/taida.lock` を更新します。

### `ingot install`

| オプション | 説明 |
|---|---|
| `--force-refresh` | `~/.taida/store/` の該当パッケージを破棄して再展開 |
| `--no-remote-check` | リモート確認をスキップ |
| `--allow-local-addon-build` | prebuild 不在時にローカルの `cargo build` へフォールバック |

挙動:
- 解決できた依存をインストールし、`.taida/taida.lock` を生成または更新します。
- アドオン依存は `native/addon.toml` の `[library.prebuild]` に従い、SHA-256 検証付きで prebuild を取得します。
- アドオンキャッシュは `~/.taida/addon-cache/` に置かれます。

ストア sidecar と stale 検知:

| キャッシュ済み sidecar | リモート取得結果 | 挙動 |
|---|---|---|
| なし | 取得成功 | 念のため再展開 |
| あり (commit_sha が実 SHA) | 一致 | 高速スキップ |
| あり (commit_sha が空) | 取得成功 | 念のため再展開 |
| あり | 不一致 | 再展開 |
| あり | 取得失敗 | スキップ + stderr に警告 |
| なし | 取得失敗 | スキップ + 強い警告 |

### `ingot update`

`ingot install` と同様ですが、世代解決時にリモート優先モードで更新を確認します。解決できた依存を再インストールし、`.taida/taida.lock` を更新します。

### `ingot publish`

`ingot publish` はタグの push のみを行います。

1. `packages.tdm` のアイデンティティ (`<<<@version owner/name`) を検証
2. 前回のリリースタグと HEAD の公開 API 差分から次バージョンを決定
3. `git tag <version>` + `git push origin <tag>`

CLI はローカルビルド、`addon.lock.toml` の生成、`native/addon.toml` / `packages.tdm` の書き換え、GitHub リリースの作成、リリースアセットのアップロードのいずれも行いません。リリース作成はアドオン側 CI の責務です。

自動バージョン繰り上げ:

| 差分 | 次バージョン | 例 |
|---|---|---|
| 前回タグなし | `a.1` | なし → `a.1` |
| 公開シンボルの削除 / 改名 | 世代繰り上げ | `a.3` → `b.1` |
| 公開シンボルの追加 | 番号繰り上げ | `a.3` → `a.4` |
| エクスポート不変の内部変更 | 番号繰り上げ | `a.3` → `a.4` |

| オプション | 説明 |
|---|---|
| `--label <LABEL>` | 自動決定したバージョンの末尾にラベルを付与 |
| `--force-version <VERSION>` | 自動判定を無視してバージョンを固定 |
| `--retag` | 既存タグを強制上書き |
| `--dry-run` | 変更を行わず計画のみ表示 |

### `ingot cache`

```bash
taida ingot cache
taida ingot cache clean [--addons]
taida ingot cache clean --store [--yes]
taida ingot cache clean --store-pkg <org>/<name>
taida ingot cache clean --all [--yes]
```

- 引数なしの `taida ingot cache` はヘルプを表示し、終了コード `0` で終了します。
- `clean` は WASM ランタイムキャッシュを削除します。
- `--addons` はアドオン prebuild キャッシュを削除します。
- `--store` は `~/.taida/store/` 全体を整理します。
- `--store-pkg <org>/<name>` は特定パッケージの全バージョンを削除します。
- `--all` は WASM + アドオンキャッシュ + ストアをまとめて整理します。
- 非 TTY 環境で `--store` / `--all` を使う場合は `--yes` が必須です。

---

## `taida doc generate`

```bash
taida doc generate [-o OUTPUT] <PATH>
```

ドキュメントコメントから Markdown を生成します。`<PATH>` にはファイルまたはディレクトリを指定できます。

---

## `taida lsp`

```bash
taida lsp
```

Tokio ランタイム上で LSP サーバーを起動します（stdio トランスポート）。

---

## `taida auth`

```bash
taida auth <login|logout|status>
```

- `login`: GitHub Device Authorization Flow を開始し、トークンを保存します。
- `logout`: 保存済みトークンを削除します。
- `status`: 現在の認証状態を表示します。

---

## `taida community`

```bash
taida community <posts|post|messages|message|author>
```

- `posts`: 公開投稿を一覧表示します。
- `post`: 認証済みユーザーとして公開投稿を作成します。
- `messages`: 自分宛の公開メッセージ一覧を取得します。
- `message`: `--to <user>` を使って公開メッセージを送信します。
- `author`: 著者プロフィールを表示します。

`taida c` のエイリアスは `@e.X` で削除されました。`taida community` を使ってください。

---

## `taida upgrade`

```bash
taida upgrade [--check] [--gen GEN] [--label LABEL] [--version VERSION]
```

Taida 本体のセルフアップデート専用コマンドです。旧 AST 書き換えフラグ (`--d28`, `--d29`, `--e30`) は `@e.X` で削除され、移行コマンドは提供しません。

| オプション | 説明 |
|---|---|
| `--check` | 更新有無の確認のみ |
| `--gen <GEN>` | 特定の世代でフィルタ |
| `--label <LABEL>` | 特定のラベルでフィルタ |
| `--version <VERSION>` | 特定のバージョンに固定 |

`--gen` と `--label` は併用できます。`--version` は `--gen` / `--label` とは排他です。
