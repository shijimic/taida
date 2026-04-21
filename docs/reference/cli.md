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
| `upgrade` | taida 自身のセルフアップデート |

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
  - **WASM SIMD (`simd128`) 有効化ポリシー (C21-3)**: 生成される `.wasm` が
    要求する WebAssembly feature は profile ごとに異なります。
    clang への `-msimd128` 指定は以下の通り：

    | profile | `-msimd128` | 生成 wasm の feature 要求 | 推奨 runtime |
    |---|---|---|---|
    | `wasm-min` | なし | simd128 非要求。**後方互換の最小バイナリ** | simd128 非対応 runtime でも実行可 |
    | `wasm-wasi` | あり | `simd128` 要求 | wasmtime 42+（default enabled） |
    | `wasm-edge` | あり | `simd128` 要求 | Cloudflare Workers 等 edge runtime（simd128 対応済） |
    | `wasm-full` | あり | `simd128` 要求 | wasmtime 42+ |

    `-msimd128` が付与された profile では LLVM の wasm auto-vectorizer が
    f32/f64 の hot loop を `v128.*` / `f32x4.*` / `f64x2.*` に降ろすことが
    許可されます（実際に SIMD 命令が出るかは LLVM の判断）。simd128 feature
    非対応の実行環境がターゲットなら `--target wasm-min` を選んでください。
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
taida install [--force-refresh | --no-remote-check] [--allow-local-addon-build]
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--force-refresh` | - | `~/.taida/store/` の該当パッケージを破棄して再展開します (C17 以降; 従来のアドオンキャッシュ無視挙動も維持) |
| `--no-remote-check` | - | commit SHA の remote 確認を skip し、既存 sidecar の内容だけを根拠に skip/refresh を判定します。offline / rate-limited 環境向け (C17) |
| `--allow-local-addon-build` | - | プレビルド不在時にローカルの `cargo build` にフォールバックします |

`--force-refresh` と `--no-remote-check` は排他です。両方を指定すると CLI が即エラーで拒否します。

挙動:
- `packages.tdm` をカレントから親方向に探索します。
- 解決できた依存を install し、`.taida/taida.lock` を生成/更新します。
- アドオン依存は `native/addon.toml` の `[library.prebuild]` に従い、SHA-256検証付きでプレビルドをダウンロードします。
- ダウンロードは `~/.taida/addon-cache/` にキャッシュされます（`taida cache clean --addons` で削除）。
- 一部依存が解決不能でも lockfile 生成は試行し、最後に終了コード1を返します。

### C17: store sidecar とステイル検知

C17 以降、`taida install` は `~/.taida/store/<org>/<name>/<version>/` の直下に `_meta.toml` sidecar を書き、次回以降の install で以下の decision table を順に評価します。

| cached sidecar | remote 取得結果 | 挙動 |
|---|---|---|
| 無し | 取得成功 | pessimistic refresh (`missing sidecar`) |
| 有り (commit_sha が実 SHA) | 一致 | fast-path skip |
| 有り (commit_sha が空) | 取得成功 | pessimistic refresh (`sidecar has no recorded commit sha`) |
| 有り | 一致しない | refresh (`remote moved: sha <old>..<new>`) — tag 再配信 (retag) を検知 |
| 有り | 取得失敗 | skip + stderr `offline, cannot verify staleness` |
| 無し | 取得失敗 | skip + stderr strong warning (`--force-refresh` を案内) |

どの経路も `taida install` の成功パス標準出力は変わりません。警告や refresh ログはすべて stderr に出るため、既存スクリプトは影響を受けません。

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
taida publish [--label LABEL] [--force-version VERSION] [--retag] [--dry-run]
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--label <LABEL>` | - | 次 version の末尾に pre-release ラベルを付与します（例: `a.4.rc`） |
| `--force-version <VERSION>` | - | 自動判定を無視して指定 version を使います（エスケープハッチ） |
| `--retag` | - | 既存 tag を強制上書きして re-push します |
| `--dry-run` | - | 実際の変更を行わず、計画だけ表示します |

### C14 tag-only 設計

`taida publish` は **tag push のみ** を行う簡潔な CLI です。以下の 3 ステップだけを実行し、即座に終了します:

1. `packages.tdm` の identity (`<<<@version owner/name`) を検証
2. 前回 release tag と HEAD の公開 API 差分から次 version を自動決定（または `--force-version` で明示）
3. `git tag <version>` + `git push origin <tag>`

CLI は以下の一切を行いません:

- `cargo build` / `cargo test` 等の local ビルド
- `addon.lock.toml` や `prebuild-targets.toml.txt` の生成
- `native/addon.toml` / `packages.tdm` の書き換え
- `git commit` / `git push origin main` / その他 `main` への書き込み
- `gh release create` や release asset の upload

release 作成の責務は addon 側 CI (`.github/workflows/release.yml`) が排他的に持ちます。Release author は `github-actions[bot]` に統一され、CLI 実行者個人のアカウントで release が作られることはありません。本体 (`taida-lang/taida`) の `release.yml` と対称な 4-job (Prepare / Gate / Build / Publish) 構造を持ち、5 platform matrix で cdylib を build して 8 件の asset (5 cdylib + `addon.lock.toml` + `prebuild-targets.toml.txt` + `SHA256SUMS`) を attach します。

`taida publish` は tag push 完了後に即 exit します。CI 完了を待つことはありません。

### 事前条件

以下が揃っていない場合は publish を reject します:

- working tree が clean（未コミット変更なし）
- `origin` remote が GitHub URL を指す
- `packages.tdm` の identity が qualified `owner/name` 形式
- `gh auth status` が active session を返す
- `--retag` なしで `origin/<next-version>` tag が未存在

### 自動 version bump ルール

前回 release tag と HEAD の `taida/*.td` export シンボル集合を比較し、次 version を決定します（`src/pkg/api_diff.rs`）:

| 差分 | 次 version | 例 |
|---|---|---|
| 前回 tag なし（初回） | `a.1` 固定 | なし → `a.1` |
| シンボル削除 or 改名 | 世代繰り上げ | `a.3` → `b.1` |
| シンボル追加のみ | 番号繰り上げ | `a.3` → `a.4` |
| 内部変更のみ（exports 不変） | 番号繰り上げ | `a.3` → `a.4` |

`--force-version` または `--retag` を指定した場合、API diff snapshot は完全に bypass されます（parser 側の `E1616` 等が pre-C13 package で擬陽性を起こすのを避けるため）。この場合 plan 出力には `API diff: skipped (force-version)` / `API diff: skipped (retag)` が表示されます。

### `--label` の挙動

`--label rc` を渡すと、自動判定（または `--force-version` で明示）された version の末尾にラベルが付与されます:

- 自動判定で `a.4` が決定 + `--label rc` → `a.4.rc`
- `--force-version a.5` + `--label rc` → `a.5.rc`

### `--retag` の挙動

`--retag` は、既に push されている tag を force-replace します。通常 publish は tag 衝突を検知すると reject しますが、release CI の設定ミスや workflow ファイルの差し替えなど、同一 version で再度 CI を走らせたい場合のために用意された escape hatch です。

内部では local tag 削除 → `git tag <version>` → `git push origin +refs/tags/<version>` を実行します。API diff は skip されます。

### `--dry-run` の出力例

```text
$ taida publish --dry-run
Publish plan for taida-lang/terminal:
  Last release tag: a.1
  API diff: added 2
  Next version: a.2
  Tag to push: a.2
  Remote: origin
  Dry-run: no git changes performed.
```

`--force-version` / `--retag` を併用した場合:

```text
$ taida publish --force-version a.5.rc --retag --dry-run
Publish plan for taida-lang/terminal:
  Last release tag: a.1
  API diff: skipped (force-version)
  Next version: a.5.rc
  Tag to push: a.5.rc
  Remote: origin
  Retag: yes (will force-replace existing tag)
  Dry-run: no git changes performed.
```

### `packages.tdm` の canonical surface

C14 以降、publish する package は identity を qualified 形式で宣言する必要があります:

```taida
>>> ./main.td
<<<@a.1 owner/name @(hello, greet)
```

- `>>> ./main.td` はエントリポイントのみを宣言します。
- `<<<@version owner/name @(symbols)` はバージョン、パッケージ ID、公開 API を一括で宣言します。
- directory 名フォールバックは C14 で廃止されました — bare `<<<@a.1` 形式は publish が reject します。

以下の旧 surface は廃止されています:

- `<<<@version owner/name => @(symbols)` (arrow 形式、@b.11.rc3 で廃止)
- `>>> ./main.td => @(symbols)` の split surface (@b.11.rc3 で廃止)
- `<<<@version @(symbols)` (symbols-only、package ID なし、@b.11.rc3 で廃止)
- `--target rust-addon` (@c.14.rc1 で廃止 — publish は target 非依存の tag push のみ)
- `--dry-run=plan` / `--dry-run=build` (@c.14.rc1 で廃止 — `--dry-run` は plan 表示のみ)
- `TAIDA_PUBLISH_SKIP_RELEASE` 環境変数 (@c.14.rc1 で廃止 — release 作成自体が CLI scope 外)

### Examples

```bash
# 次 version を確認 (dry-run)
taida publish --dry-run

# 実 publish — tag push のみ行い、CI が release を作成する
taida publish

# pre-release ラベル付き
taida publish --label rc

# 自動判定を無視して明示 version で publish
taida publish --force-version a.5

# 既存 tag を force-replace
taida publish --force-version a.5 --retag
```

---

## `taida cache`

```bash
taida cache clean [--addons]
taida cache clean --store [--yes]                # C17
taida cache clean --store-pkg <org>/<name>        # C17
taida cache clean --all [--yes]                   # C17 で store を含むよう拡張
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--addons` | - | アドオンプレビルドのキャッシュ (`~/.taida/addon-cache/`) を削除します |
| `--store` | - | C17: `~/.taida/store/` 全体を prune します。サマリを表示してから確認プロンプト (TTY でない場合は `--yes` 必須) |
| `--store-pkg <org>/<name>` | - | C17: 特定パッケージの全 version を store から削除します (確認プロンプトなし) |
| `--all` | - | WASM + addon-cache + store の全てを一括 prune します (C17 以降) |
| `--yes` | `-y` | `--store` / `--all` のプロンプトを自動で yes 応答します (CI 向け) |

挙動:
- `taida cache clean` は WASM runtime cache を削除します (既存挙動)。
- `--addons` でアドオンキャッシュのみ。
- `--store` は店 (`~/.taida/store/`) 全体の prune で、事前にパッケージ数と合計サイズのサマリを表示し確認します。`--store-pkg <org>/<name>` は単一パッケージに絞り、確認プロンプトを出しません。
- `--store-pkg` は `--store` / `--all` と併用できません (排他エラー)。
- 非 TTY (パイプ、CI) で `--store` / `--all` を使う場合は `--yes` が必須です。未指定だとエラーで中断します (store を誤って全消去しないためのガード)。

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

---

## `taida upgrade`

```bash
taida upgrade [--check] [--gen GEN] [--label LABEL] [--version VERSION]
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--check` | - | 更新有無の確認のみ（インストールしない） |
| `--gen <GEN>` | - | 特定の世代（generation）でフィルタ（例: `b`） |
| `--label <LABEL>` | - | 特定のラベルでフィルタ（例: `rc2`） |
| `--version <VERSION>` | - | 特定バージョンに固定（例: `@b.10.rc2`） |

挙動:
- GitHub Releases API から全リリースタグを取得し、バージョン解決を行います。
- デフォルトでは最新の stable バージョン（ラベルなし、または `stable` ラベル）に更新します。
- `--gen` と `--label` は組み合わせ可能です。`--version` は `--gen`/`--label` と排他です。
- 同じ `@gen.num` に stable が複数ある場合、ラベルなしを優先します（例: `@b.11` > `@b.11.stable`）。
- 更新時はプラットフォームに応じたアーカイブ（`.tar.gz`）をダウンロードし、リリースの `SHA256SUMS` ファイルで整合性を検証してからバイナリを展開・自己置換します。`SHA256SUMS` が見つからない場合は警告を表示し、検証をスキップします。
- `--check` 指定時は更新有無を表示するのみで、exit code は常に `0` です。

制限:
- **Windows**: `--check`（更新有無の確認）のみ対応。自己置換（`.zip` 展開 + バイナリ置換）は未実装です。
- `update`（依存パッケージ更新）と `upgrade`（taida 自身の更新）は別のコマンドです。
