# Taida CLI リファレンス

このページは現行 Taida CLI の単一リファレンスです。`taida` トップレベル
コマンドは `taida way` / `taida ingot` 配下のサブコマンド群に整理されて
います。タグごとの land 履歴は `CHANGELOG.md` を参照してください。

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

### プロジェクトルート解決

`taida` / `taida build` / `taida way` / `taida ingot` などサブコマンドは、引数で渡された `<PATH>` から親方向に **プロジェクトルートマーカー** を探索して解決します。受理されるマーカーは次の 3 種だけです:

- `packages.tdm` (パッケージマニフェスト)
- `taida.toml` (将来予約)
- `.git/` (git リポジトリルート)

`.taida/` ディレクトリはプロジェクトルートマーカーには **含まれません**。
`packages.tdm` / `taida.toml` / `.git` のいずれも持たないディレクトリは
「プロジェクトルートではない」と扱われ、CLI はファイル単独実行モードとして
フォールバックします。`.taida/` はビルドキャッシュ (`build/`, `deps/`,
`taida.lock`) の格納先として使われますが、ルート判定の trigger には
なりません。`packages.tdm` を持たない単発スクリプト実行でプロジェクト
ルート扱いが必要な場合は、明示的に空の `packages.tdm` を置いてください
(詳細は [`docs/guide/10_modules.md`](../guide/10_modules.md) の
「packages.tdm」セクションを参照)。

---

## 公開トップレベルコマンド

| コマンド | 用途 |
|---|---|
| `taida` / `taida <FILE>` | REPL / インタプリタ実行 |
| `build` | Native / WASM 成果物の生成 |
| `way` | 品質ハブ。check / lint / verify / todo を集約 |
| `graph` | 構造グラフの抽出 / summary / query |
| `doc` | ドキュメントコメントから Markdown を生成 |
| `ingot` | パッケージ・依存ハブ。deps / install / update / publish / cache を集約 |
| `init` | プロジェクト雛形の生成 |
| `lsp` | LSP サーバーの起動 |
| `auth` | 認証状態の管理 |
| `community` | コミュニティ API へのアクセス |
| `upgrade` | Taida 本体のセルフアップデート |

`check` / `verify` / `lint` / `todo` / `inspect` / `transpile` / `compile` / `deps` / `install` / `update` / `publish` / `cache` / `c` のトップレベルコマンドは提供されていません。これらを叩いた場合は `[E1700]` と移行先を表示し、終了コード `2` で終了します。

| 旧コマンド | 移行先 |
|---|---|
| `taida check` | `taida way check`、または完全実行の `taida way` |
| `taida verify` | `taida way verify` |
| `taida lint` | `taida way lint` |
| `taida todo` | `taida way todo` |
| `taida inspect` | `taida graph summary` |
| `taida transpile` | `taida build native` |
| `taida compile` | `taida build native` |
| `taida deps` | `taida ingot deps` |
| `taida install` | `taida ingot install` |
| `taida update` | `taida ingot update` |
| `taida publish` | `taida ingot publish` |
| `taida cache` | `taida ingot cache` |
| `taida c` | `taida community` |

---

## `taida build`

`taida build` には **単一ターゲットビルド** と **ディスクリプタビルド** の 2 つのモードがあります。第一位置引数を unit 名や plan 名として再利用しないことで、両モードの曖昧さを排除します。

### 単一ターゲットビルド (既存互換)

```bash
taida build [native|wasm-min|wasm-wasi|wasm-edge|wasm-full] [--release] [--no-cache] [--diag-format text|jsonl] [-o OUTPUT] [--entry ENTRY] [--handler SYMBOL] <PATH>
```

| オプション | 短縮 | 説明 |
|---|---|---|
| `--output <PATH>` / `--outdir <DIR>` | `-o` | 出力先 |
| `--entry <PATH>` | - | Native + dir入力時のエントリ上書き |
| `--handler <SYMBOL>` | - | Native / WASM handler mode の entry 関数名 |
| `--release` | `-r` | TODO/Stub 残存時に失敗 |
| `--no-cache` | - | WASM runtime object cache を使わずにビルド |
| `--diag-format <text\|jsonl>` | - | 診断出力形式 |

挙動:
- ターゲットは位置引数で指定します。省略時は `native` です。
- `taida build native <PATH>` は Native バイナリを生成します。
- `taida build wasm-* <PATH>` は `.wasm` を生成します。
- `--handler <SYMBOL>` を指定すると、`taida-lang/abi` の `WebRequest` /
  `WebResponse` handler としてビルドします。Native は標準入力/標準出力の
  JSON bridge を持つ binary、WASM は host adapter から呼び出す ABI export
  を持つ module を生成します。Host capability を使う WASM handler は
  `start` / `poll` / `resume` の session ABI で host call を待ち、adapter が
  resume JSON を返します。`wasm-edge --handler` は Workers 用 glue と、
  同じ stem の `.edge.js` を生成します。詳細は [`docs/api/abi.md`](../api/abi.md)
  を参照してください。
- 旧 `js` ターゲットは移行期間の互換機能です。正式なリリースパリティ契約には含めず、新規コードは Native または WASM を選んでください。
- `--target <target>` / `--target=<target>` フラグは提供していません。位置引数で指定してください。誤って指定した場合は `[E1700]` + exit 2 になります。
- 既定では parse + type check を実行します。`--no-check` で型検査をスキップできます。
- `--diag-format jsonl` を指定するとコンパイル診断を `taida.diagnostic.v1` JSONL 形式で出力します。

#### 診断 JSON / JSONL の `null` について

`taida.diagnostic.v1` 系の JSON / JSONL 出力で省略可能フィールド (例: 解析途中で取れなかった位置情報、optional な `hint` フィールド) が **JSON null** として現れることがあります。これは **機械可読な JSON schema レベルの「フィールド未提供」マーカー** であり、**Taida 言語の値ではありません**。

- Taida 言語仕様としては `null` / `undefined` / `none` / `nil` / `unit` / `void` は値にも名前にも存在しません (`docs/reference/naming_conventions.md` 参照)。
- 診断 JSON に出る `null` は、外部ツール (エディタ、SARIF パイプライン、CI) が Taida コードを直接読まずに診断結果だけを処理するための機械可読フォーマット上の慣習です。
- Taida コード側で `null` をハンドリングする経路は存在しません。`Lax[T]` / `Result[T, P]` / 共通 Enum で「値の不在」を意味のある値として表現してください。

将来 schema が変更され null を完全に排除する場合は、`taida.diagnostic.v2` 等のメジャー番号でアナウンスします。`v1` を消費するツールは null を「fieldは未提供」として扱ってください。

### ディスクリプタビルド

```bash
taida build <PATH> --unit <NAME> [--run-hooks]
taida build <PATH> --plan <NAME> [--run-hooks]
taida build <PATH> --all-units [--run-hooks]
taida --no-check build <PATH> --unit <NAME>
taida --no-check build <PATH> --plan <NAME>
taida --no-check build <PATH> --all-units
```

| 形 | 意味 |
|----|------|
| `--unit <NAME>` | `<PATH>` 内で export された `BuildUnit` のうち `name == <NAME>` のものだけをビルド |
| `--plan <NAME>` | `<PATH>` 内で export された `BuildPlan` のうち `name == <NAME>` をビルド |
| `--all-units` | `<PATH>` 内で export された全 `BuildUnit` をビルド (単独 export と `BuildPlan` 経由のどちらも対象) |
| `--run-hooks` | ディスクリプタに接続された `BuildHook.before` を実行 |

ディスクリプタビルドは、`taida-lang/build` パッケージのディスクリプタ (`BuildUnit` / `BuildPlan` / `AssetBundle` / `RouteAsset` / `BuildHook`) をランタイム値としてではなく **ビルドディスクリプタ** として扱います。ビルド成功時は `.taida/build/` にターゲット別成果物、静的アセット、hook ログ、`artifact-map.json` をトランザクション単位でコミットします。詳細は `docs/reference/build_descriptors.md` を参照してください。

`--no-check` はディスクリプタファイルの parse とディスクリプタ抽出には影響しません。`--unit` / `--plan` / `--all-units` のいずれでも、選択された `BuildUnit` と依存 `BuildUnit` の子ビルドへ伝播し、単一ターゲットビルドと同じように各エントリソースの型検査をスキップします。

#### 禁止される組み合わせ (`E1900〜E1909`)

| 組み合わせ | 結果 |
|------------|------|
| 位置引数の `<target>` と `--unit` / `--plan` / `--all-units` の併用 | `[E1900]` で拒否 |
| `--unit` と `--plan` の併用 | `[E1901]` で拒否 |
| `--unit` と `--all-units` の併用 | `[E1901]` で拒否 |
| `--plan` と `--all-units` の併用 | `[E1901]` で拒否 |
| `--unit` / `--plan` / `--all-units` と `--entry` の併用 | `[E1900]` で拒否 |
| `--unit` / `--plan` / `--all-units` 無しで `--run-hooks` を指定 | `[E1900]` で拒否 |
| `--unit` / `--plan` / `--all-units` を指定したが `<PATH>` に export 済みディスクリプタが無い | `[E1902]` でビルドを中断 |
| `--unit <NAME>` を指定したが該当する `BuildUnit` が無い | `[E1903]` でビルドを中断 (候補名一覧を表示) |
| `--plan <NAME>` を指定したが該当する `BuildPlan` が無い | `[E1904]` でビルドを中断 |

`<PATH>` の解釈は単一ターゲットビルドと共通です。第一位置引数はビルドルートのソースパス (ファイル / ディレクトリ / プロジェクトルート) であり、unit 名や plan 名として解釈しません。

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
taida ingot install [--force-refresh | --no-remote-check] [--allow-local-addon-build] [--allow-fresh] [--frozen]
taida ingot migrate-lockfile
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
| `--allow-fresh` | third-party addon prebuild の release age policy をこの実行だけ明示 override |
| `--frozen` | `.taida/taida.lock` の `(name, version, integrity)` triple と解決結果が一致しない場合に失敗 |

挙動:
- 解決できた依存をインストールし、`.taida/taida.lock` を生成または更新します。
- 通常の `install` でも既存 lockfile と解決結果の triple mismatch は `[E32K2_LOCKFILE_INTEGRITY_MISMATCH]` で拒否します。依存更新は `taida ingot update`、旧 lockfile 変換は `taida ingot migrate-lockfile` を使います。
- アドオン依存は `native/addon.toml` の `[library.prebuild]` に従い、SHA-256 検証付きで prebuild を取得します。
- third-party addon prebuild は、GitHub Release の `published_at` が既定の cooling-off window を満たすまで拒否されます。既定は `taida-lang/*` が `0d`、それ以外が `3d` です。
- release age は CLI override、`packages.tdm` の `[security] min_release_age = "3d"`、グローバル `~/.taida/config.toml` の `[security] min_release_age`、環境変数 `TAIDA_MIN_RELEASE_AGE`、組み込み既定の順に解決します。`--allow-fresh` を使った実行は `.taida/install-audit.log` に記録します。
- addon lock entry は target、SHA-256、release `published_at`、publisher login を記録します。publisher 変更や `published_at` の後退は hard fail です。
- アドオンキャッシュは `~/.taida/addon-cache/` に置かれます。

### `ingot migrate-lockfile`

`.taida/taida.lock` schema v1 / `fnv1a:` integrity を、installed `.taida/deps` tree から再計算した現行 schema / `sha256:` integrity に書き換えます。通常の `install` は旧 schema と `fnv1a:` を `[E32K2_LOCKFILE_V1_REJECTED]` で拒否します。migration 中に installed dependency が見つからない場合は `[E32K2_LOCKFILE_MIGRATE_FAIL]` で停止します。

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
2. `packages.tdm` のファサードと entry モジュールの公開 API が一致することを検証
3. 前回のリリースタグと HEAD の公開 API 差分から次バージョンを決定
4. `git tag <version>` + `git push origin <tag>`

CLI はローカルビルド、`addon.lock.toml` の生成、`native/addon.toml` / `packages.tdm` の書き換え、GitHub リリースの作成、リリースアセットのアップロードのいずれも行いません。リリース作成はアドオン側 CI の責務です。

facade 検証では、`packages.tdm` の `<<<@version owner/name @(symbols)` と entry module の `<<< @(symbols)` を双方向に比較します。entry module が export していない symbol を facade に書いた場合、または entry module が export している symbol を facade に書き忘れた場合、publish は tag を作成する前に拒否されます。

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

`taida c` の短縮エイリアスは提供していません。`taida community` を使ってください。

---

## `taida upgrade`

```bash
taida upgrade [--check] [--gen GEN] [--label LABEL] [--version VERSION]
```

Taida 本体のセルフアップデート専用コマンドです。AST 書き換えフラグ (`--d28`, `--d29`, `--e30` 等の旧 alias) は提供していません。バージョン間の手動移行は CHANGELOG を参照してください。

| オプション | 説明 |
|---|---|
| `--check` | 更新有無の確認のみ |
| `--gen <GEN>` | 特定の世代でフィルタ |
| `--label <LABEL>` | 特定のラベルでフィルタ |
| `--version <VERSION>` | 特定のバージョンに固定 |

`--gen` と `--label` は併用できます。`--version` は `--gen` / `--label` とは排他です。

`taida upgrade` は自己置換を行うため、release provenance は hard-fail 契約です。Release asset に
`SHA256SUMS` が存在しない場合、または対象 archive の行が存在しない場合は `[E32K1_UPGRADE_NO_SHA256SUMS]`
で停止します。`SHA256SUMS` は cosign keyless verification で `taida-lang/taida` の release workflow
identity に pin され、検証後に archive bytes の SHA-256 と照合されます。`TAIDA_GITHUB_API_URL` は
production upgrade path では読まれず、release metadata の取得先は `https://api.github.com` 固定です。

production upgrade path はダウンロード URL について `https://` 以外の scheme を `[E32K1_UPGRADE_NON_HTTPS_URL]`
で hard-fail します。release metadata 改竄経由で `file://` などの URL が渡される攻撃を防ぐ defense-in-depth
ガードであり、テスト経路だけが `download_bytes_for_test` ヘルパーを通じて `file://` を受け付けます。

ダウンロードした archive / `SHA256SUMS` / cosign bundle のステージング先は `~/.taida/cache/upgrade/`
(ディレクトリ mode `0700`) に固定されます。Unix では `HOME` から `~/.taida/cache/upgrade/`
までを `O_NOFOLLOW` 付きの dirfd traversal で開き、管理下の各ディレクトリを開いた fd に対して
owner / mode 検査します。各ファイルも cache dirfd から `O_NOFOLLOW | O_EXCL` で原子作成され、
`/tmp` 配下の予測可能パスは使いません。アップグレードを開始する直前に cache ディレクトリ経路も
検査され、symlink・他 UID 所有・group / world bits 立ちのいずれかに該当する場合は
`[E32K1_UPGRADE_STAGE_FAILED]` で hard-fail します。

`sudo` で `taida upgrade` を実行する場合、`HOME` を共有書き込み可能ディレクトリに上書きしないでください。
cache ディレクトリは `HOME` (Windows では `USERPROFILE`) を起点に解決されます。Unix では
`HOME` 自体も現在の実効 UID 所有の symlink でないディレクトリであることを確認します。
検証ツールは staged file のパスを再度開くため、同じ UID で `HOME` 配下を書き換えられるプロセスは
信頼境界内として扱います。共有書き込み可能な `HOME` は指定しないでください。

`install.sh` の `TAIDA_VERIFY_SIGNATURES` 既定値は `required` です。`taida upgrade` の cosign 必須
契約と `install.sh` の trust model はここで一致し、明示的に `TAIDA_VERIFY_SIGNATURES=best-effort` /
`off` を指定したオペレーターだけが緩いモードに落ちます。
