# ビルドディスクリプタリファレンス

> 関連 API: [`docs/api/build_descriptors.md`](../api/build_descriptors.md)
> 関連 CLI: [`docs/reference/cli.md::taida build`](cli.md)

Native / JS / WASM の混合ビルドを記述するための仕組みです。Native サーバー
成果物が wasm 成果物や静的アセットバンドルを「ビルド成果物」として参照する
構成で、通常のランタイム import とビルド成果物依存を混同しないグラフを提供
します。

各ディスクリプタ型 (`BuildUnit` / `BuildPlan` / `AssetBundle` / `RouteAsset` /
`BuildHook`) のシグネチャ・フィールド・例は API リファレンスを参照して
ください。本リファレンスでは、ビルドドライバが扱う **概念** と **動作仕様**
(成果物グラフ、依存閉包、トランザクション、診断、ゲート) を扱います。

## 1. ディスクリプタとランタイム値の境界

ディスクリプタ式は **ビルドドライバ専用値** であり、通常のランタイムに
降ろされません。型検査器とビルドドライバは `taida-lang/build` から来た
値が以下の文脈以外で使われた場合、診断を出します。

- トップレベル export (`<<< serverX`、`<<< plan`) による named artifact entrypoint
- 他ディスクリプタのフィールド (`BuildPlan.units` / `BuildUnit.assets` /
  `RouteAsset.unit` / `RouteAsset.asset` / `<ディスクリプタ>.before`)
- ビルドドライバの ディスクリプタ import

`stdout(serverX)`、ユーザ関数引数への通常の受け渡し、`Str[serverX]()` などは
すべてビルドドライバ専用値の濫用としてエラーになります。

ディスクリプタは `packages.tdm` ではなく、ディスクリプタ module の中で組み立てます。
`packages.tdm` は依存宣言と公開 API マニフェストの責務に留めます。

## 2. CLI 連携

`taida build` の `--unit` / `--plan` / `--all-units` がディスクリプタビルドの
入口です。CLI フラグの完全な定義は
[`docs/reference/cli.md::taida build`](cli.md) を参照してください。

第一位置引数を unit 名や plan 名として再利用しないことで、単一ターゲット
ビルドとディスクリプタビルドの曖昧さを排除しています。

## 3. 成果物グラフ (artifact graph)

ビルドドライバはディスクリプタとモジュール依存から、5 種類の有向エッジを
持つグラフを組み立てます。

| エッジ種別 | 起点 | 終点 | 降ろし方 |
|-----------|------|------|---------|
| `NormalImport` | モジュール | モジュール / パッケージシンボル | 現成果物の依存閉包に入る |
| `DescriptorImport` | ビルドルートモジュール | export 済みディスクリプタ | ビルドドライバが読む。ランタイム symbol import ではない |
| `ArtifactDependency` | `BuildUnit` | `BuildUnit` | ビルド順序に使う。ランタイム import としては降ろさない |
| `AssetDependency` | `BuildUnit` / `BuildPlan` | `AssetBundle` | コピー順序とアセットマップに使う |
| `HookDependency` | ディスクリプタ | `BuildHook` | hook の明示有効化時の実行順に使う |

`RouteAsset(path, unit)` は `ArtifactDependency` を、
`RouteAsset(path, asset)` は `AssetDependency` を作ります。ビルド順序は
成果物 / アセット / hook の有向グラフでトポロジカル順に決め、循環は
ビルド計画段階で `[E1940]` になります。

成果物依存はソースモジュールの通常 import 依存とは別種です。Native 成果物に
wasm 専用コードを取り込むことはありません。

## 4. ターゲット別依存閉包

各 `BuildUnit` は `entry` シンボルを起点に通常 import 依存閉包を計算します。
成果物依存先の `BuildUnit` 閉包は依存元の閉包に混ぜません。

ターゲット互換性は閉包単位で検証します。

- ターゲットと互換しないコアパッケージ API / OS 操作 / ネットサーバー API は `[E1941]` で reject
- アドオン互換メタデータも閉包検証の入力にする
- 共有モジュールはターゲット中立なら複数閉包に入ってよい
- 共有モジュールがターゲット専用 API を使う場合、その API と互換しない `BuildUnit` の閉包では reject
- コンパイラによる自動サブグラフ分割 / ブランチ推論は対象外

ターゲット別のコア API 互換性表は
[`docs/api/build_descriptors.md §7`](../api/build_descriptors.md) に集約しています。

## 5. ディスクリプタ名の安全性

`BuildUnit.name` / `BuildPlan.name` / `AssetBundle.name` / `BuildHook.name` は
staging 階層、artifact-map のキー、ホックログディレクトリで単一パスセグメント
としてそのまま使われます。プロジェクトルートを越える書き出しを防ぐため、
ビルドドライバは parser 直後に以下のいずれかを満たす値を `[E1916]` で
hard-fail します。

- 空文字
- `.` または `..`
- `/` または `\` を含む値
- 見た目が `/` や `.` に近い Unicode confusable を含む値
  (`∕` U+2215, `⁄` U+2044, `⧸` U+29F8, `／` U+FF0F, `․` U+2024, `．` U+FF0E)
- 先頭が `.` で始まる隠しセグメント
- NUL バイト (`\0`) を含む値
- Windows 予約デバイス名 (`CON`, `PRN`, `AUX`, `NUL`, `COM1`〜`COM9`,
  `LPT1`〜`LPT9`)。拡張子付き (`CON.txt` など) も予約名として reject

`frontend.v1` のようなドット中段やハイフン / アンダースコアを含む値は
受理します。

## 6. AssetBundle の安全性ポリシー

`AssetBundle` はコピーとアセットマップのみを担当します。バンドル化や
最適化は行わず、必要なら `BuildHook` で外部ツールを明示的に有効化した
うえで実行し、その出力ディレクトリを取り込みます。

- `root` はプロジェクトルート判定マーカ (`packages.tdm` / `taida.toml` / `.git`) 配下に限定 (`[E1910]`)
- `files` は `root` からの相対グロブのみ受理。絶対パス / `..` / `~` 展開は reject (`[E1911]`)
- グロブ展開後のパスが `root` のプレフィックスでない場合 `[E1912]`
- 通常ファイル以外 (ディレクトリ / シンボリックリンク / デバイス / FIFO / ソケット) は `[E1913]`
- シンボリックリンクは既定で辿らない。ドット始まりは既定で除外
- 異なるソースが同一の正規化済み出力パスへ解決される場合 `[E1914]`
- `RouteAsset.path` の重複は `[E1915]`

詳細なフィールド仕様は
[`docs/api/build_descriptors.md §3`](../api/build_descriptors.md) を参照。

## 7. BuildHook の実行ポリシー

- `BuildHook` は既定で実行しません。`taida build --run-hooks` が必須
- `cwd` がプロジェクトルート配下にない場合 `[E1950]`
- hook が付与されているのに `--run-hooks` が無い場合 `[E1951]`
- hook が非ゼロ終了 / シグナルで失敗した場合 `[E1952]` でビルド全体が fail し、既存の正常ビルド出力は保持される
- ビルドドライバは hook の起動前に親プロセスの環境変数を全てクリア (`env_clear()`)、固定許可リスト (`PATH`, `HOME`, `LANG`, `LC_ALL`) のみ引き継ぐ
- hook ログは `.taida/build/hooks/<hook-name>/<transaction-id>.log` に append-only で書き込まれる。同じ hook が 1 つのトランザクション内で複数回実行された場合、2 回目以降は `<transaction-id>-<ordinal>.log` になる
- hook ログの保持期間や件数上限は自動管理しない。CI などで長期運用する場合は `.taida/build/hooks/` を必要に応じて手動で prune する

ネットワーク接続を伴うコマンド (`npm ci` 等) も特別扱いしません。実行は
`--run-hooks` の明示指定が必須であり、リリース / CI ゲートでは hook ログと
指紋を検証対象とします。

## 8. `.taida/build` のトランザクショナル更新

ビルドドライバはプロジェクトルート (`packages.tdm`, `taida.toml`, `.git` の
いずれかを持つ祖先ディレクトリ) の `.taida/build/.tmp-<transaction-id>/` を
ステージング領域として使います。プロジェクトルートが見つからない ディスクリプタ
build は `[E1902]` で中断します。`transaction-id` はプロセス ID と高精度
時刻から作り、ステージングディレクトリは既存パスを許容しない排他作成で
確保します。

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
  .lock                       # 実行中 ディスクリプタ build の排他 lock
  .tmp-<transaction-id>/       # ステージング領域
    native/<unit-name>/
    ...
    transaction.json
```

### 8.2 コミットとロールバック

- すべての `BuildUnit` / `AssetBundle` がステージングへ書き込み、すべて成功したときだけ `<target>/<unit-name>/` 単位で `rename`(2) (POSIX) または `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` (Windows) でアトミック置換
- `BuildHook` ログは append-only の監査ログとして hook 実行時に `.taida/build/hooks/` へ `create_new` で作成する。成果物コミット時に hook ディレクトリ全体を置換しない
- 旧ディレクトリはステージング配下に `.replaced-<transaction-id>/` として一時退避し、コミット後に削除
- 失敗時はステージングを削除し、既存のコミット済み領域には触れない
- アトミック置換に対応しないファイルシステム (一部のネットワーク共有など) では `[E1924]` でビルドを中断する。暗黙の代替経路は提供しない

### 8.3 トランザクション ID

- 同一ビルドから生成された各成果物 / アセットマップ / hook ログには同一の `<transaction-id>` を埋め込む
- `BuildUnit` と `AssetBundle` のコミット済みディレクトリには `.transaction-id` sidecar を書く
- `artifact-map.json` には `transaction_id`, `committed_at`, `selectors`, `build_mode` を含める
- 起動時の取り残しステージング掃除: ビルドドライバ起動時に `transaction.json` を読み、24 時間超過 / プロセス ID が現存しないステージングを削除し `.cleanup.log` に記録する (記録なしの削除は行わない)。プロセス生存判定は Linux で `/proc/<pid>` の存在、macOS / *BSD で `kill(pid, 0)` の戻り値 (`ESRCH` で死亡判定、`EPERM` は別ユーザの live 扱い) を用いる。プロセス生存判定が未対応のホスト (現状 Windows) では TTL を 4 時間に短縮し、`.cleanup.log` の各スキャン行に `pid_alive_check=unsupported ttl=4h` を残す。`transaction.json` の mtime ベースの heartbeat は本リリースでは未実装のため、極端に長時間 (>4h) のビルドが未対応ホストで進行中だと、別セッションが起動した cleanup スキャンによって active staging dir が誤って削除される可能性がある
- 並列ビルドは `.taida/build/.lock` を既存パスを許容しない排他作成で確保する。衝突したプロセスは lock 内の PID を診断に含めて `[E1923]` でビルドを中断する。lock 内 PID が既に死んでいると判定できる場合は、起動時に stale lock を削除してから取り直す

## 9. 診断スキーマ

ビルドドライバ由来の診断は jsonl (`--diag-format jsonl`) レコードに `build`
ブロックを付与します。

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

ソースのみの診断はこの `build.*` ブロックを持ちません。コンシューマは
`build` の有無でソース由来 / ビルド由来を区別できます。

テキスト出力はビルドドライバ由来のときに行を追加します。

```text
error[E1605]: comparison ...
  --> src/main.td:42:8
  unit=server-x target=native
  edge=ArtifactDependency dependency=server-x -> frontend-a
```

`taida graph` のソースグラフ既存 JSON スキーマは破壊しません。成果物グラフは
別メタデータ版 (`artifact_graph_version`) で出力します。

## 10. Native サーバー + wasm 経路ゲート

必須ゲートとループバック煙テストを分けます。

### 10.1 必須ゲート (`cargo test` 経由)

- ディスクリプタビルドを実行し、`.taida/build/{target}/{unit}/` の生成物ツリーを比較するスナップショットフィクスチャ
- `artifact-map.json` のスキーマ検証
- 重複経路 / 重複出力パス / ターゲット閉包違反でビルドが中断することを確認する負のテスト
- `.taida/build/{target}/` の出力分離確認
- 実ネットワークでの bind / ポート確保 / ループバック権限を要求するテストは含めない

### 10.2 ループバック煙テスト (専用 CI ジョブ)

- bind: `127.0.0.1` 固定。ポートはエフェメラル (`bind 0` を OS に割り当てさせる)
- HTTP GET の応答ボディの SHA-256 が、コミット済み成果物または `AssetBundle` のバイト列と一致することを確認
- タイムアウト: サーバー起動 5 秒、1 リクエスト 10 秒、全体 30 秒
- スキップ禁止: ループバック bind が不可能な環境ではジョブ自体を失敗扱いにする
- 再試行なし。失敗時はサーバーログとクライアントトレースを CI 成果物としてアップロードする

ビルドドライバ自体は HTTP の配信意味論を持ちません。サーバーコードまたは
web パッケージがアセットマップを読んで配信します。

## 11. 関連ドキュメント

- [`docs/api/build_descriptors.md`](../api/build_descriptors.md) — 各ディスクリプタ型の API シグネチャと例
- [`docs/reference/cli.md`](cli.md) — `taida build` の CLI 仕様
- [`docs/reference/diagnostic_codes.md`](diagnostic_codes.md) — `E1900〜E1959` 帯
- [`docs/guide/10_modules.md`](../guide/10_modules.md) — `packages.tdm` の責務
