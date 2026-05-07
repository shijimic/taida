# ビルドディスクリプタリファレンス

> 対象パッケージ: `taida-lang/build`
> 関連 CLI: `docs/reference/cli.md::taida build`

複数バックエンド混合ビルドのディスクリプタ仕様です。Native サーバー成果物が wasm 成果物や静的アセットバンドルを「ビルド成果物」として参照する構成で、通常のランタイム import とビルド成果物依存を混同しないグラフを提供します。

## 1. パッケージとインポート

```taida
>>> taida-lang/build => @(BuildUnit, BuildPlan, AssetBundle, RouteAsset, BuildHook)
```

`BuildUnit` / `BuildPlan` / `AssetBundle` / `RouteAsset` / `BuildHook` は core-bundled パッケージ `taida-lang/build` の公開 API です。コンパイラの組み込み型ではありません。ただし型検査器とビルドドライバはこのパッケージから来たディスクリプタファミリを **ディスクリプタ専用値** として認識し、通常のランタイムへ降ろさないようにします。

`packages.tdm` に `BuildUnit` / `BuildPlan` / `AssetBundle` / `BuildHook` を直接書きません。`packages.tdm` は依存宣言と公開 API マニフェストの責務に留めます。

## 2. ディスクリプタの形

### 2.1 最小フィールド

| ディスクリプタ | 必須 | 任意 | 意味 |
|----------------|------|------|------|
| `BuildUnit` | `name: Str`, `target: Str`, `entry: Symbol` | `assets: @[RouteAsset]`, `before: @[BuildHook]` | コンパイル成果物のディスクリプタ |
| `BuildPlan` | `name: Str`, `units: @[BuildUnit]` | `assets: @[AssetBundle]`, `before: @[BuildHook]` | ビルドルートをまとめるディスクリプタ |
| `AssetBundle` | `name: Str`, `root: Str`, `files: @[Str]` | `output: Str`, `before: @[BuildHook]` | コピー専用の静的アセットディスクリプタ |
| `RouteAsset` | `path: Str`、`unit` か `asset` のいずれか | `name: Str` | 成果物・アセットバンドルへの経路メタデータ |
| `BuildHook` | `name: Str`, `command: Str`, `cwd: Str` | `env: @[@(name: Str, value: Str)]` | ビルド時前段ステップのディスクリプタ |

### 2.2 例

```taida
>>> ./frontend.td => @(aMain)
>>> ./server.td => @(xMain)

frontendA <= BuildUnit(
  name <= "frontend-a",
  target <= "wasm-edge",
  entry <= aMain
)

frontendAssets <= AssetBundle(
  name <= "frontend-assets",
  root <= "./nextjs-app/out",
  files <= @["**/*"],
  output <= "assets/frontend"
)

serverX <= BuildUnit(
  name <= "server-x",
  target <= "native",
  entry <= xMain,
  assets <= @[
    RouteAsset(path <= "/a.wasm", unit <= frontendA),
    RouteAsset(path <= "/", asset <= frontendAssets)
  ]
)

plan <= BuildPlan(
  name <= "web-release",
  units <= @[serverX, frontendA]
)

<<< serverX
<<< frontendA
<<< plan
```

`BuildUnit.entry` は **ローカル import で取り込んだ関数シンボル** を指します。ファイルパスエントリは採用しません。実装上も `>>> ./server.td => @(serverMain)` のような descriptor module 内のローカル import から `entry <= serverMain` を解決し、その import 元ファイルを対象ターゲットでビルドします。ソースファイルやディレクトリは CLI のビルドルートパスで与え、ディスクリプタ内ではシンボル境界を使います。

`BuildPlan` は必須ではありません。export された `BuildUnit` 単体もビルドルートとして許可されます。複数 unit を一括でビルドする場合や、リリースゲートのルートを明示する場合に `BuildPlan` を使います。

### 2.3 ディスクリプタが有効な文脈

ディスクリプタ値は以下の文脈でのみ有効です。

- トップレベル export (`<<< serverX`、`<<< plan`) による named artifact entrypoint
- `BuildPlan.units`
- `BuildUnit.assets`
- `RouteAsset.unit` / `RouteAsset.asset`
- `BuildUnit.before` / `AssetBundle.before` / `BuildPlan.before`
- ビルドドライバの descriptor import

それ以外でランタイム値として使う (`stdout(serverX)`、ユーザ関数引数への通常の受け渡し、`Str[serverX]()` など) と、型検査器またはビルドドライバが診断を出します。

## 3. CLI 連携

CLI 側の表記は `docs/reference/cli.md::taida build` を参照してください。ディスクリプタビルド (`--unit` / `--plan` / `--all-units`) は本ドキュメントのディスクリプタ前提で動作します。第一位置引数を unit 名や plan 名として再利用しません。

## 4. 成果物グラフ (artifact graph)

| エッジ種別 | 起点 | 終点 | 降ろし方 |
|-----------|------|------|---------|
| `NormalImport` | モジュール | モジュール / パッケージシンボル | 現成果物の依存閉包に入る |
| `DescriptorImport` | ビルドルートモジュール | export 済みディスクリプタ | ビルドドライバが読む。ランタイム symbol import ではない |
| `ArtifactDependency` | `BuildUnit` | `BuildUnit` | ビルド順序に使う。ランタイム import としては降ろさない |
| `AssetDependency` | `BuildUnit` / `BuildPlan` | `AssetBundle` | コピー順序とアセットマップに使う |
| `HookDependency` | ディスクリプタ | `BuildHook` | hook の明示有効化時の実行順に使う |

`RouteAsset(path, unit)` は `ArtifactDependency` を作ります。`RouteAsset(path, asset)` は `AssetDependency` を作ります。ビルド順序は成果物 / アセット / hook の有向グラフでトポロジカル順に決め、循環はビルド計画段階で決定的な診断 (`E1940`) になります。

成果物依存はソースモジュールの通常 import 依存とは別種です。Native 成果物に wasm 専用コードを取り込むことはありません。

## 5. ターゲット別依存閉包

各 `BuildUnit` は `entry` シンボルを起点に通常 import 依存閉包を計算します。成果物依存先の `BuildUnit` 閉包は依存元の閉包に混ぜません。

ターゲット互換性は閉包単位で検証します。

- ターゲットと互換しないコアパッケージ API / OS 操作 / ネットサーバー API は reject (`E1941`)
- アドオン互換メタデータも閉包検証の入力にする
- 共有モジュールはターゲット中立なら複数閉包に入ってよい
- 共有モジュールがターゲット専用 API を使う場合、その API と互換しない `BuildUnit` の閉包では reject
- コンパイラによる自動サブグラフ分割 / ブランチ推論は対象外

ターゲット別のコア API 互換性は以下です。表にない外部パッケージは、通常の依存解決とビルド対象バックエンドの能力に従います。

| target | `taida-lang/os` | `taida-lang/net` | `taida-lang/terminal` |
|--------|-----------------|------------------|-----------------------|
| `js` | 受理 | 受理 | 受理 |
| `native` | 受理 | 受理 | 受理 |
| `wasm-min` | reject | reject | reject |
| `wasm-edge` | `EnvVar`, `allEnv` のみ受理 | reject | reject |
| `wasm-wasi` | `EnvVar`, `allEnv`, `Read`, `Exists`, `writeFile`, `readBytesAt` のみ受理 | reject | reject |
| `wasm-full` | `wasm-wasi` と同じ OS subset を受理 | reject | reject |

allow list は import 元のシンボル名で判定します。alias は判定を変えません。コア API import でシンボルリストが空の場合は package wildcard とみなし、制限付きターゲットでは互換性のない import として扱います。

## 5.5 ディスクリプタ名の単一パスセグメント制約

`BuildUnit.name` / `BuildPlan.name` / `AssetBundle.name` / `BuildHook.name` は staging 階層、artifact-map のキー、ホックログディレクトリで単一パスセグメントとしてそのまま使われます。プロジェクトルートを越える書き出しを防ぐため、ビルドドライバは `parse_build_*` 直後に以下のいずれかを満たす値を `[E1916]` で hard-fail します。

- 空文字
- `.` または `..`
- `/` または `\\` を含む値
- 先頭が `.` で始まる隠しセグメント
- NUL バイト (`\0`) を含む値

例: `name <= "../../../../tmp/pwn"` は `[E1916]` で reject されるため、ステージング段階でプロジェクトルート外に書き出されることはありません。`name <= "frontend.v1"` のようなドット中段やハイフン / アンダースコアを含む値は受理します。

## 6. AssetBundle のパス安全性

`AssetBundle` はコピーとアセットマップのみを担当します。minify / バンドル / トランスパイル / 画像最適化は行いません (必要なら `BuildHook` で外部ツールを明示的に有効化したうえで実行し、その出力ディレクトリを `AssetBundle.root` として取り込みます)。

`output` を省略した場合、出力先は `.taida/build/assets/<bundle-name>/` です。`output <= "assets/frontend"` のように指定した場合は `.taida/build/assets/frontend/` にコピーします。

### 6.1 プロジェクトルート閉じ込め

- `AssetBundle.root` はプロジェクトルート配下に限定 (プロジェクトルート判定マーカは `packages.tdm` / `taida.toml` / `.git`)
- `root` を正規化し、プロジェクトルートの正規化パスのプレフィックスにならないものを reject (`E1910`)
- シンボリックリンクを辿った後も同じ判定を行う

### 6.2 グロブの制約

- `files` は `AssetBundle.root` からの相対グロブのみ受け付ける。絶対パス / `..` 区切り / `~` 展開は reject (`E1911`)
- グロブ展開後の各候補について以下のいずれかで reject:
  - 正規化パスが `AssetBundle.root` のプレフィックスでない (`E1912`)
  - 通常ファイル以外 (ディレクトリエントリ / シンボリックリンク / デバイスファイル / FIFO / ソケット) (`E1913`)
- シンボリックリンクは既定で辿らない。リンク自体を値とするエントリは `E1913` で reject
- ドット始まりの隠しファイルは既定で除外。含めたい場合は `**/.*` のように明示する

### 6.3 出力パスの衝突

- 異なるソースが同一の正規化済み出力パスに解決される場合、ビルド計画段階でビルドを中断 (`E1914`)
- `RouteAsset.path` の衝突は `E1915` (同じサーバー unit 内の重複経路)

### 6.4 将来の強化候補 (現時点では未強制)

- ファイルサイズ上限、グロブ件数上限、バンドル合計サイズ上限は将来世代で再評価

## 7. BuildHook

### 7.1 既定では実行しない

- `BuildHook` は既定で実行しません。実行にはディスクリプタビルドで `--run-hooks` を明示的に指定する必要があります。
- 初回確認プロンプトは採用しません。CI / 自動化で再現可能にするため、意図は CLI フラグとビルドメタデータに残します。

### 7.2 cwd と失敗の扱い

- `cwd` は正規化後にプロジェクトルート配下でなければなりません (`E1950`)
- `BuildHook` が付与されているのに `--run-hooks` が無い場合は `E1951` で中断します
- hook ログには `command`, `cwd`, `env` 名、`exit_code`, 標準出力 / 標準エラー、hook 設定指紋を含めます
- hook ログは `.taida/build/hooks/<hook-name>/` に累積します。過去のトランザクションのログは、新しいビルドのコミットで置換されません
- 同じ hook が 1 つのトランザクション内で複数回実行された場合、最初のログは `<transaction-id>.log`、2 回目以降は `<transaction-id>-<ordinal>.log` になります
- hook ログは一時ファイルへ書き切ってから同じディレクトリ内で rename します。中断時に隠し一時ファイルが残ることはありますが、未完了ログを通常の `<transaction-id>.log` として公開しません
- hook ログの保持期間や件数上限は自動管理しません。CI などで長期運用する場合は、必要な監査期間に合わせて `.taida/build/hooks/` を手動 prune してください
- hook が非ゼロ終了 / シグナルで失敗した場合は `E1952` でビルド全体が fail し、既存の正常ビルド出力は保持されます (本書 8 のトランザクショナル更新)

### 7.3 環境変数の隔離

- ビルドドライバは hook の起動前に親プロセスの環境変数を全てクリアします (`env_clear()`)
- 以下の固定許可リストのみ親プロセスから引き継ぎます: `PATH`, `HOME`, `LANG`, `LC_ALL`
- それ以外の環境変数は `BuildHook.env` で明示的に宣言してください
- 許可リストはコマンドの解決とロケール非依存ツールの最低要件を満たすための最小集合であり、再現性確保のため将来世代でも縮小方向にしか変更しません
- 本世代の許可リストは POSIX 環境を前提にしています。Windows ホストでは `cmd /C` の動作に必要な `SystemRoot` / `ComSpec` / `PATHEXT` / `TEMP` 系を含めるか、`BuildHook.env` で明示宣言する追加対応が必要であり、本リリースでは未検証です

### 7.4 ネットワーク

ネットワーク接続を伴うコマンド (`npm ci` 等) もビルドドライバは特別扱いしません。ただし実行は `--run-hooks` の明示指定が必須であり、リリース / CI ゲートでは hook ログと指紋を検証対象とします。

## 8. `.taida/build` のトランザクショナル更新

ビルドドライバは `.taida/build/.tmp-<transaction-id>/` をステージング領域として使います。`transaction-id` はプロセス ID と高精度時刻から作り、ステージングディレクトリは既存パスを許容しない排他作成で確保します。

### 8.1 配置

```text
.taida/build/
  artifact-map.json            # コミット済みマップ (transaction id を含む)
  native/<unit-name>/          # コミット済み成果物 (ターゲット別)
    .transaction-id
  wasm-edge/<unit-name>/
  assets/<bundle-name>/
    .transaction-id
  hooks/<hook-name>/<transaction-id>.log
  hooks/<hook-name>/<transaction-id>-2.log
  .tmp-<transaction-id>/       # ステージング領域
    native/<unit-name>/
    ...
    transaction.json
```

### 8.2 コミットとロールバック

- すべての `BuildUnit` / `AssetBundle` がステージングへ書き込み、すべて成功したときだけ `<target>/<unit-name>/` 単位で `rename`(2) (POSIX) または `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` (Windows) でアトミック置換
- `BuildHook` ログは append-only の監査ログとして hook 実行時に `.taida/build/hooks/` へ `create_new` で作成します。成果物コミット時に hook ディレクトリ全体を置換しません
- 旧ディレクトリはステージング配下に `.replaced-<transaction-id>/` として一時退避し、コミット後に削除
- 失敗時はステージングを削除し、既存のコミット済み領域には触れない
- アトミック置換に対応しないファイルシステム (一部のネットワーク共有など) では `[E1924]` でビルドを中断します。暗黙の代替経路は提供しません

### 8.3 トランザクション ID

- 同一ビルドから生成された各成果物 / アセットマップ / hook ログには同一の `<transaction-id>` を埋め込みます
- `BuildUnit` と `AssetBundle` のコミット済みディレクトリには `.transaction-id` sidecar を書きます
- `artifact-map.json` には `transaction_id`, `committed_at`, `selectors`, `build_mode` を含めます
- 起動時の取り残しステージング掃除: ビルドドライバ起動時に `transaction.json` を読み、24 時間超過 / プロセス ID が現存しないステージングを削除し `.cleanup.log` に記録します (記録なしの削除は行いません)。プロセス生存判定は Linux で `/proc/<pid>` の存在、macOS / *BSD で `kill(pid, 0)` の戻り値 (`ESRCH` で死亡判定、`EPERM` は別ユーザの live 扱い) を用います。プロセス生存判定が未対応のホスト (現状 Windows) では TTL を 4 時間に短縮し、`.cleanup.log` の各スキャン行に `pid_alive_check=unsupported ttl=4h` を残します。`transaction.json` の mtime ベースの heartbeat は本リリースでは未実装のため、極端に長時間 (>4h) のビルドが未対応ホストで進行中だと、別セッションが起動した cleanup スキャンによって active staging dir が誤って削除される可能性があります
- 並列ビルドはステージング作成を `O_EXCL` で排他し、衝突したプロセスは `[E1923]` でビルドを中断します

## 9. 診断スキーマ

ビルドドライバ由来の診断は jsonl (`--diag-format jsonl`) レコードに `build` ブロックを付与します。

| フィールド | 型 | 必須 | 意味 |
|-----------|----|------|------|
| `build.unit` | `Str \| null` | 必須 | `BuildUnit` 名 (ビルド計画レベルの診断は `null`) |
| `build.target` | `Str \| null` | 必須 | ターゲット文字列 (`"native"` / `"wasm-edge"` など) |
| `build.edge_kind` | `"NormalImport" \| "DescriptorImport" \| "ArtifactDependency" \| "AssetDependency" \| "HookDependency" \| null` | 任意 | グラフのエッジ違反時のみ付与 |
| `build.dependency_path` | `@[Str] \| null` | 任意 | 循環 / 閉包違反の依存連鎖 |
| `build.transaction_id` | `Str \| null` | 必須 | 本書 8.3 のトランザクション ID |
| `build.hook_name` | `Str \| null` | 任意 | `BuildHook` 診断の hook 名 |
| `build.cwd` | `Str \| null` | 任意 | `BuildHook` 診断の実行 cwd |
| `build.exit_code` | `Int \| null` | 任意 | 子ビルドまたは hook の終了コード |

ソースのみの診断はこの `build.*` ブロックを持ちません。コンシューマは `build` の有無でソース由来 / ビルド由来を区別できます。

テキスト出力はビルドドライバ由来のときに行を追加します。

```text
error[E1605]: comparison ...
  --> src/main.td:42:8
  unit=server-x target=native
  edge=ArtifactDependency dependency=server-x -> frontend-a
```

`taida graph` のソースグラフ既存 JSON スキーマは破壊しません。成果物グラフは別メタデータ版 (`artifact_graph_version`) で出力します。

## 10. Native サーバー + wasm 経路ゲート

必須ゲートとループバック煙テストを分けます。

### 10.1 必須ゲート (`cargo test` 経由)

- ディスクリプタビルドを実行し、`.taida/build/{target}/{unit}/` の生成物ツリーを比較するスナップショットフィクスチャ
- `artifact-map.json` のスキーマ検証
- 重複経路 / 重複出力パス / ターゲット閉包違反でビルドが中断することを確認する負のテスト
- `.taida/build/{target}/` の出力分離確認
- 実ネットワークでの bind / ポート確保 / ループバック権限を要求するテストは含めない

### 10.2 ループバック煙テスト (専用スクリプトと専用 CI ジョブ)

- 実行コマンド: `tests/run_e32_loopback_smoke.sh`
- bind: `127.0.0.1` 固定。ポートはエフェメラル (`bind 0` を OS に割り当てさせる)
- HTTP GET の応答ボディの SHA-256 が、コミット済み成果物または `AssetBundle` のバイト列と一致することを確認
- タイムアウト: サーバー起動 5 秒、1 リクエスト 10 秒、全体 30 秒
- スキップ禁止: ループバック bind が不可能な環境ではジョブ自体を失敗扱いにします
- 再試行なし。失敗時はサーバーログとクライアントトレースを CI 成果物としてアップロードします

ビルドドライバ自体は HTTP の配信意味論を持ちません。サーバーコードまたは web パッケージがアセットマップを読んで配信します。

## 11. 関連ドキュメント

- `docs/reference/cli.md` — `taida build` の CLI 仕様
- `docs/reference/diagnostic_codes.md` — `E1900〜E1959` 帯
- `docs/guide/10_modules.md` — `packages.tdm` の責務
