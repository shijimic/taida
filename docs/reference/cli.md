# Taida CLI リファレンス

> 更新日: 2026-04-10  
> 実装正本: `src/main.rs`

このページは Taida CLI の単一リファレンスです。  
コマンド仕様は実装 (`src/main.rs`) に合わせています。

---

## 実行モード

### 1. REPL

```bash
taida
taida --no-check
```

- 引数なしで REPL を起動します。
- `--no-check` を付けると REPL 内の型チェックをスキップします。

### 2. ファイル実行（Interpreter）

```bash
taida <FILE>
taida --no-check <FILE>
```

- サブコマンド以外の第1引数はファイルパスとして扱われます。
- `--no-check` で型チェックをスキップできます。

### 3. 共通フラグ

`--no-check` はグローバルに受理され、サブコマンド振り分け前に除去されます。

- 実際に効果がある: `taida <FILE>`, `taida` (REPL), `taida build`, `taida transpile`
- 現状効果がない: `graph`, `verify`, `inspect`, `init`, `deps`, `install`, `update`, `doc`, `todo`, `lsp`
- `--help` / `-h` はトップレベルヘルプを表示します。
- `--version` / `-V` はバージョンを表示します。

### 4. トップレベルヘルプ

```bash
taida --help
taida -h
taida help
```

挙動:
- 使用方法、主要サブコマンド、グローバルフラグを表示します。
- exit code は `0` です。
- `taida <COMMAND> --help` のようなサブコマンド個別ヘルプへの導線を含みます。

---

## コマンド一覧

| コマンド | 用途 |
|---|---|
| `build` | 成果物生成の主コマンド（`--target native|js|wasm-*`） |
| `transpile` | `build --target js` のエイリアス |
| `todo` | `TODO[...]` 抽出と集計 |
| `check` | parse/type/verify(error-coverage) の静的診断 |
| `graph` | グラフ抽出 / summary / query |
| `verify` | 構造検証 |
| `inspect` | summary + graph統計 + verify 一括表示 |
| `init` | プロジェクト雛形生成 |
| `deps` | 依存解決 + install + lockfile（strict） |
| `install` | 依存インストール + lockfile生成 |
| `update` | リモート優先で依存更新 + lockfile更新 |
| `publish` | package publish の準備と push |
| `doc generate` | doc comments から Markdown 生成 |
| `lsp` | LSP サーバー起動（stdio） |
| `auth` | 認証状態の管理 |
| `community` | community API へのアクセス |

補足:
- 表のサブコマンドは `--help` / `-h` を受理し、各節の usage を stdout に出して exit code `0` で終了します。
- `auth` / `community` の下位 verb も `taida auth login --help`, `taida community posts --help` のように個別 help を受理します。
- 実行モードは `taida <FILE>` であり、独立した `taida run` subcommand は現状ありません。

---

## `taida build`

```bash
taida build [--target native|js|wasm-min|wasm-wasi|wasm-edge|wasm-full] [--release] [--diag-format text|jsonl] [-o OUTPUT] [--entry ENTRY] <PATH>
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--target <native\|js\|wasm-min\|wasm-wasi\|wasm-edge\|wasm-full>` | - | 生成ターゲット（既定: `native`） |
| `--output <PATH>` / `--outdir <DIR>` | `-o` | 出力先（file入力時はファイル、dir入力時はディレクトリ） |
| `--entry <PATH>` | - | Native + dir入力時のエントリ上書き（既定: `main.td`） |
| `--release` | `-r` | TODO/Stub 残存時に失敗 |
| `--diag-format <text\|jsonl>` | - | 診断出力形式（既定: `text`） |

挙動:
- `<PATH>` は file/dir を受理します。
- `--target` 省略時は `native` がデフォルトです。
- `--target native`（デフォルト）:
  - file入力: そのファイルをエントリに Native バイナリ生成（既定 `src/foo.td -> src/foo`）
  - dir入力: 既定エントリ `<PATH>/main.td`、`--entry` で上書き可能
- `--target js`:
  - file入力: 単一 `.mjs` を生成（既定 `src/foo.td -> src/foo.mjs`）
  - dir入力: `.td` を再帰収集して `.mjs` を出力（既定出力は `<PATH>` 親の `dist/`）
  - dir入力で `package.json` が無ければ `{ "type": "module" }` を生成します。
- `--target wasm-*`:
  - `.wasm` 成果物を生成します。
  - 対応ターゲットは `wasm-min`, `wasm-wasi`, `wasm-edge`, `wasm-full` です。
- 既定では parse + type check を実行します（`--no-check` で type check をスキップ）。
- `--release` では `TODO` / `Stub` を検出すると終了します（file入力時は import 依存も再帰走査）。
- `--diag-format jsonl` では compile 診断を `taida.diagnostic.v1` JSONL で出力します（parse/type/verify/codegen/io）。

---

## `taida transpile`

```bash
taida transpile [--release] [--diag-format text|jsonl] [-o OUTPUT] <PATH>
```

`taida build --target js` のエイリアスです。引数はそのまま `build` に転送されます。

---

## `taida todo`

```bash
taida todo [--format text|json] [PATH]
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--format <FORMAT>` | `-f` | `text` (既定) / `json` |

挙動:
- `PATH` 省略時は `.` を対象にします。
- ディレクトリ指定時は `.td` を再帰収集します。
- JSON 出力は `total`, `todos`, `byId`, `byFile` を返します。

---

## `taida check`

```bash
taida check [--json] <PATH>
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--json` | - | 機械可読 JSON (`schema: taida.check.v1`) で出力 |

挙動:
- `<PATH>` はファイルまたはディレクトリ指定に対応します（ディレクトリ時は `.td` を再帰収集）。
- parse/type/verify(`error-coverage`) を実行し、`summary.total/errors/warnings/info/files` を出力します。
- `errors > 0` の場合は exit code `1`、それ以外は `0` です。

### Front Gate（仕様の正門）としての役割

`taida check` は単なる型検査器ではなく、**仕様の正門（front gate）** として機能します。

- **未実装機能の reject 口**: 言語仕様として確定しているが未実装の機能、または明示的に廃止された機能は、`taida check` が backend（Interpreter / JS / Native）に到達する前に拒否します。これにより backend ごとに別々の壊れ方をすることを防ぎます。
- **確定済み意味論の強制**: 関数オーバーロード禁止（`[E1501]`）、旧 `_` 部分適用禁止（`[E1502]`）、TypeDef 部分適用禁止（`[E1503]`）、パイプライン外 `Mold[_]()` 直接束縛禁止（`[E1504]`）など、確定済みの意味論制約を checker が強制します。
- **`--no-check` との関係**: `--no-check` で型チェックをスキップしても、parser error（`E01xx` / `E02xx`）は回避できません。checker が拒否する構文（`E13xx` / `E14xx` / `E15xx`）は `--no-check` でバイパスされますが、その場合 backend での挙動は未定義です。
- **診断コード**: `taida check` が発行する全ての診断コードは `docs/reference/diagnostic_codes.md` に定義されています。AI ツールはコードを安定識別子として利用できます。

---

## `taida graph`

```bash
taida graph [--type TYPE] [--format FORMAT] <PATH>
taida graph summary [--type TYPE] [--format FORMAT] <PATH>
taida graph query --type TYPE --query EXPR <PATH>
taida graph --help
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--type <TYPE>` | `-t` | `dataflow` / `module` / `type-hierarchy` / `error` / `call` |
| `--format <FORMAT>` | `-f` | `text` / `json` / `mermaid` / `dot` |
| `--query <EXPR>` | - | `query` サブコマンド用 |

挙動:
- 現状の `<PATH>` は単一ファイル入力です。
- 無効な `--type` / `--format` はエラー終了します。
- `summary` は構造サマリ(JSON文字列)を出力します。
- `--help` / `-h` は graph 専用の usage / type / format 一覧を表示し、exit code `0` で終了します。

---

## `taida verify`

```bash
taida verify [--check CHECK] [--format FORMAT] <PATH>
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--check <CHECK>` | `-c` | 実行チェック（複数指定可） |
| `--format <FORMAT>` | `-f` | `text` (既定) / `json` / `jsonl` / `sarif` |

`<PATH>` はファイルまたはディレクトリ指定に対応します。  
ディレクトリ指定時は `.td` を再帰走査します。

### `--check` の値

| チェック | 重要度 | 概要 |
|---|---|---|
| `error-coverage` | error | throw サイトのカバレッジ検証 |
| `no-circular-deps` | error | モジュール循環依存検出 |
| `dead-code` | warning | 到達不能関数検出 |
| `type-consistency` | error | 型階層の循環検出 |
| `unchecked-division` | warning | 除算安全性チェック枠（現行構文では通常 finding なし） |
| `direction-constraint` | error | 単一方向制約（`=>`/`<=`, `]=>`/`<=[`） |
| `unchecked-lax` | warning | Lax 未検査利用検出 |
| `naming-convention` | warning | 命名規約違反検出 |

注:
- 不明な check 名は `Unknown check` の warning として出力されます。
- `--severity` は現状未実装です。
- `--format jsonl` では finding 行 + summary 行を出力し、`ERROR` finding がある場合は exit code `1` になります。

---

## `taida inspect`

```bash
taida inspect [--format text|json|sarif] <PATH>
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--format <FORMAT>` | `-f` | `text` (既定) / `json` / `sarif` |

挙動:
- 現状の `<PATH>` は単一ファイル入力です。
- `text`: summary + 各グラフ統計 + verify結果を表示します。
- `json`: `summary` と `verification`（SARIF）を1つの JSON にまとめて出力します。
- `sarif`: verify 結果のみを SARIF で出力します。

---

## `taida init`

```bash
taida init [--target rust-addon] [DIR]
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--target rust-addon` | - | Rust アドオンプロジェクトをスキャフォールドします |

挙動:
- `DIR` 省略時はカレントディレクトリに初期化します。
- `packages.tdm` が既にある場合は失敗します。
- 通常モード: `packages.tdm`, `main.td`（未存在時）, `.taida/` を作成します。
- `--target rust-addon`: `Cargo.toml`, `src/lib.rs`, `native/addon.toml`, `taida/<name>.td`, `README.md` を含む Rust アドオンプロジェクトをスキャフォールドします。

---

## `taida deps`

```bash
taida deps
```

挙動:
- `packages.tdm` をカレントから親方向に探索します。
- 依存解決エラーが1件でもある場合、install/lockfile更新を行わず失敗します（strict fail-fast）。
- 成功時は `.taida/deps` と `.taida/taida.lock` を更新します。

---

## `taida install`

```bash
taida install [--force-refresh] [--allow-local-addon-build]
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--force-refresh` | - | アドオンキャッシュを無視して再ダウンロードします |
| `--allow-local-addon-build` | - | プレビルド不在時にローカルの `cargo build` にフォールバックします |

挙動:
- `packages.tdm` をカレントから親方向に探索します。
- 解決できた依存を install し、`.taida/taida.lock` を生成/更新します。
- アドオン依存は `native/addon.toml` の `[library.prebuild]` に従い、SHA-256検証付きでプレビルドをダウンロードします。
- ダウンロードは `~/.taida/addon-cache/` にキャッシュされます（`taida cache clean --addons` で削除）。
- 一部依存が解決不能でも lockfile 生成は試行し、最後に終了コード1を返します。

---

## `taida update`

```bash
taida update [--allow-local-addon-build]
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--allow-local-addon-build` | - | プレビルド不在時にローカルの `cargo build` にフォールバックします |

挙動:
- `install` と同様ですが、世代解決時にリモート優先モードで更新確認します。
- 解決できた依存を再 install し、`.taida/taida.lock` を更新します。

---

## `taida publish`

```bash
taida publish [--label LABEL] [--dry-run[=MODE]] [--target rust-addon]
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--label <LABEL>` | - | publish version の末尾にラベルを付与します（例: `@a.4.rc`） |
| `--dry-run` | - | 実際の変更を行わず、計画だけ表示します（`--dry-run=plan` と同等） |
| `--dry-run=plan` | - | 計画を表示して終了します |
| `--dry-run=build` | - | cargo build + lockfile まで実行し、git/release はスキップします |
| `--target rust-addon` | - | Rust cdylib アドオンとしてビルド・リリースします |

挙動:
- `taida auth login` 済みであることを要求します（author 名の解決に使用）。
- working tree が clean であることを要求します（未コミットの変更がある場合はエラー）。
- `packages.tdm` と git tag (`<version>`) から次の publish version を決定します。
- `packages.tdm` の `<<<@...` を決定した exact version に更新します。
- git commit + tag (`<version>`) + push origin を実行します。
- 成功後、`taida-community/proposals` への登録用 URL を表示します。

---

## `taida cache`

```bash
taida cache clean [--addons]
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--addons` | - | アドオンプレビルドのキャッシュ (`~/.taida/addon-cache/`) を削除します |

挙動:
- `taida cache clean` はキャッシュを削除します。
- `--addons` でアドオンキャッシュのみを対象にします。

---

## `taida doc generate`

```bash
taida doc generate [-o OUTPUT] <PATH>
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--output <PATH>` | `-o` | 出力先（省略時は標準出力） |

挙動:
- `<PATH>` はファイルまたはディレクトリ指定に対応します。
- ディレクトリ指定時は `.td` を再帰収集し、doc comments から Markdown を連結生成します。
- 現状は Markdown 出力のみです。

---

## `taida lsp`

```bash
taida lsp
```

挙動:
- Tokio runtime 上で LSP サーバーを起動します（stdio transport）。

---

## `taida auth`

```bash
taida auth <login|logout|status>
taida auth <login|logout|status> --help
```

挙動:
- `login`, `logout`, `status` を提供します。
- `taida auth login --help` などの nested help は stdout に usage を表示し、exit code `0` で終了します。
- `login` は GitHub Device Authorization Flow を開始し、成功した token を保存します。
- `logout` は保存済み token を削除します。
- `status` は現在の認証状態を表示します。

---

## `taida community`

```bash
taida community <posts|post|messages|message|author>
taida community <posts|post|messages|message|author> --help
```

挙動:
- `posts`, `post`, `messages`, `message`, `author` を提供します。
- `taida community posts --help` などの nested help は stdout に usage を表示し、exit code `0` で終了します。
- `posts` は公開投稿を一覧表示します。
- `post` は認証済みユーザーとして公開投稿を作成します。本文に `--help` のような literal flag を含めたい場合は `--` 以降を本文として扱います。
- `messages` は自分宛の公開メッセージ一覧を取得します。
- `message` は `--to <user>` を使って公開メッセージを送信します。本文に `--help` のような literal flag を含めたい場合は `--` 以降を本文として扱います。
- `author` は著者プロフィールを表示します。引数省略時は認証済みユーザー自身を表示します。
