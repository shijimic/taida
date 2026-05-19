# taida-lang/build API リファレンス

`taida-lang/build` は Native / JS / WASM 成果物の混合ビルドを記述するための
ディスクリプタ群を提供するコア同梱パッケージです。サーバー成果物が wasm
成果物や静的アセットバンドルを「ビルド成果物」として参照する構成を、
通常のランタイム import と分離したグラフで表現できます。

ビルドドライバそのものの動作 (CLI 連携、成果物グラフ、依存閉包、
トランザクション、診断スキーマ、ゲート) は
[`docs/reference/build_descriptors.md`](../reference/build_descriptors.md)
を参照してください。本書では各ディスクリプタ型の **API シグネチャ** と
**有効な文脈** に集中します。

## 0. インポート

```taida
>>> taida-lang/build => @(BuildUnit, BuildPlan, AssetBundle, RouteAsset, BuildHook)
```

`packages.tdm` に `BuildUnit` / `BuildPlan` / `AssetBundle` / `BuildHook` を
直接書きません。`packages.tdm` は依存宣言と公開 API マニフェストの責務に
留めます。コンパイラはこのパッケージから来たディスクリプタファミリを
**ディスクリプタ専用値** として認識し、通常のランタイムへ降ろさない
ように制御します。

---

## 1. BuildUnit

> コンパイル成果物 (Native binary / `.mjs` / `.wasm`) のディスクリプタ。

```taida
BuildUnit name: Str  target: Str  entry: Symbol  assets: @[RouteAsset]  before: @[BuildHook]
```

**フィールド**:

| Name | Type | 必須 | Description |
|------|------|------|-------------|
| `name` | `Str` | はい | 成果物名。staging 階層、artifact-map のキー、ホックログディレクトリで単一パスセグメントとして使われる。 |
| `target` | `Str` | はい | `"native"` / `"js"` / `"wasm-min"` / `"wasm-wasi"` / `"wasm-edge"` / `"wasm-full"` のいずれか。 |
| `entry` | `Symbol` | はい | ローカル import で取り込んだ関数シンボル。 |
| `assets` | `@[RouteAsset]` | いいえ | 成果物・アセットバンドルへの経路メタデータ。 |
| `before` | `@[BuildHook]` | いいえ | ビルド時前段ステップ。`--run-hooks` 明示時のみ実行。 |

**Constraints**:

- `entry` はファイルパスではなくシンボル。`>>> ./server.td => @(serverMain)` で取り込んだ関数を `entry <= serverMain` の形で渡す。
- `name` の単一パスセグメント制約に違反すると `[E1916]` で reject。

**Example**:

```taida
>>> ./server.td => @(serverMain)

serverX <= BuildUnit(
  name <= "server-x",
  target <= "native",
  entry <= serverMain
)

<<< serverX
```

---

## 2. BuildPlan

> 複数の `BuildUnit` をまとめるディスクリプタ。リリースゲートのルートを明示
> したい場合や、複数 unit を一括ビルドしたい場合に使う。

```taida
BuildPlan name: Str  units: @[BuildUnit]  assets: @[AssetBundle]  before: @[BuildHook]
```

**フィールド**:

| Name | Type | 必須 | Description |
|------|------|------|-------------|
| `name` | `Str` | はい | 計画名。`name` の単一パスセグメント制約は `BuildUnit` と同じ。 |
| `units` | `@[BuildUnit]` | はい | ビルド対象の `BuildUnit` リスト。 |
| `assets` | `@[AssetBundle]` | いいえ | この計画で使う静的アセット。 |
| `before` | `@[BuildHook]` | いいえ | 計画レベルの前段ステップ。 |

**AI-Context**:
`BuildPlan` は必須ではありません。export された `BuildUnit` 単体でも
ビルドルートとして許可されます。

**Example**:

```taida
plan <= BuildPlan(
  name <= "web-release",
  units <= @[serverX, frontendA]
)

<<< plan
```

---

## 3. AssetBundle

> コピー専用の静的アセットディスクリプタ。minify / バンドル / トランスパイル
> / 画像最適化は行わない。

```taida
AssetBundle name: Str  root: Str  files: @[Str]  output: Str  before: @[BuildHook]
```

**フィールド**:

| Name | Type | 必須 | Description |
|------|------|------|-------------|
| `name` | `Str` | はい | バンドル名。 |
| `root` | `Str` | はい | コピー元ディレクトリ。プロジェクトルート配下に限定。 |
| `files` | `@[Str]` | はい | `root` からの相対グロブのリスト。 |
| `output` | `Str` | いいえ | 出力先サブパス。省略時は `.taida/build/assets/<bundle-name>/`。 |
| `before` | `@[BuildHook]` | いいえ | バンドル前に実行する hook。 |

**Constraints (パス安全性)**:

- `root` はプロジェクトルート判定マーカ (`packages.tdm` / `taida.toml` / `.git`) 配下に限定。違反は `[E1910]`。
- `files` は `root` からの相対グロブのみ受理。絶対パス / `..` 区切り / `~` 展開は `[E1911]` で reject。
- グロブ展開後のパスが `root` のプレフィックスでない場合 `[E1912]`、通常ファイル以外 (ディレクトリエントリ / シンボリックリンク / デバイスファイル / FIFO / ソケット) は `[E1913]` で reject。
- 異なるソースが同一の正規化済み出力パスへ解決される場合 `[E1914]`。
- シンボリックリンクは既定で辿らない。ドット始まりのファイルは既定で除外 (含めたい場合は `**/.*` のように明示)。

**Example**:

```taida
frontendAssets <= AssetBundle(
  name <= "frontend-assets",
  root <= "./nextjs-app/out",
  files <= @["**/*"],
  output <= "assets/frontend"
)
```

---

## 4. RouteAsset

> `BuildUnit` から見た、別 unit の成果物または `AssetBundle` への経路メタデータ。

```taida
RouteAsset path: Str  unit: BuildUnit  asset: AssetBundle  name: Str
```

**フィールド**:

| Name | Type | 必須 | Description |
|------|------|------|-------------|
| `path` | `Str` | はい | サーバ側の経路 (例: `/`、`/a.wasm`)。 |
| `unit` | `BuildUnit` | `unit` か `asset` のどちらか必須 | 成果物 unit への参照。 |
| `asset` | `AssetBundle` | `unit` か `asset` のどちらか必須 | アセットバンドルへの参照。 |
| `name` | `Str` | いいえ | 経路の識別ラベル。 |

**Constraints**:

- `unit` と `asset` を同時に指定しない。
- 同じサーバー unit 内で `path` が重複する場合 `[E1915]`。

**Example**:

```taida
RouteAsset(path <= "/a.wasm", unit <= frontendA)
RouteAsset(path <= "/", asset <= frontendAssets)
```

---

## 5. BuildHook

> ビルド時前段ステップのディスクリプタ。

```taida
BuildHook name: Str  command: Str  cwd: Str  env: @[@(name: Str, value: Str)]
```

**フィールド**:

| Name | Type | 必須 | Description |
|------|------|------|-------------|
| `name` | `Str` | はい | hook 名。ログディレクトリのキー。 |
| `command` | `Str` | はい | 実行コマンド。 |
| `cwd` | `Str` | はい | 実行時の作業ディレクトリ。プロジェクトルート配下に限定。 |
| `env` | `@[@(name: Str, value: Str)]` | いいえ | hook が必要とする環境変数。 |

**実行ポリシー**:

- `BuildHook` は既定で実行しません。`taida build --run-hooks` が必須。
- `cwd` がプロジェクトルート配下にない場合 `[E1950]`。
- hook が付与されているのに `--run-hooks` が無い場合 `[E1951]`。
- hook が非ゼロ終了 / シグナルで失敗した場合 `[E1952]` でビルド全体が fail。
- ビルドドライバは hook の起動前に親プロセスの環境変数を全てクリアし、固定許可リスト (`PATH`, `HOME`, `LANG`, `LC_ALL`) のみ引き継ぎます。それ以外は `env` で明示宣言してください。

**Example**:

```taida
buildFrontend <= BuildHook(
  name <= "build-frontend",
  command <= "npm run build",
  cwd <= "./nextjs-app",
  env <= @[@(name <= "NODE_ENV", value <= "production")]
)
```

---

## 6. ディスクリプタが有効な文脈

ディスクリプタ値は以下の文脈でのみ有効です。それ以外でランタイム値として
使う (`stdout(serverX)`、ユーザ関数引数への通常の受け渡し、`Str[serverX]()`
など) と、型検査器またはビルドドライバが診断を出します。

- トップレベル export (`<<< serverX`、`<<< plan`) による named artifact entrypoint
- `BuildPlan.units`
- `BuildUnit.assets`
- `RouteAsset.unit` / `RouteAsset.asset`
- `BuildUnit.before` / `AssetBundle.before` / `BuildPlan.before`
- ビルドドライバのディスクリプタ取り込み

---

## 7. ターゲット別コア API 互換性

各 `BuildUnit` は `target` に応じて、依存閉包に含めて良いコア API を制限します。
表にない外部パッケージは通常の依存解決とビルド対象バックエンドの能力に従います。

| target | `taida-lang/os` | `taida-lang/net` | `taida-lang/terminal` |
|--------|-----------------|------------------|-----------------------|
| `js` | 受理 | 受理 | 受理 |
| `native` | 受理 | 受理 | 受理 |
| `wasm-min` | reject | reject | reject |
| `wasm-edge` | `EnvVar`, `allEnv` のみ受理 | reject | reject |
| `wasm-wasi` | `EnvVar`, `allEnv`, `Read`, `Exists`, `writeFile`, `readBytesAt` のみ受理 | reject | reject |
| `wasm-full` | `wasm-wasi` と同じ OS subset を受理 | reject | reject |

allow list は import 元のシンボル名で判定します。alias は判定を変えません。
コア API import でシンボルリストが空の場合は package wildcard とみなし、
制限付きターゲットでは互換性のない import として扱います。

ターゲット閉包違反は `[E1941]` で reject されます。

---

## 8. 完全な例

```taida
>>> ./frontend.td => @(aMain)
>>> ./server.td => @(xMain)
>>> taida-lang/build => @(BuildUnit, BuildPlan, AssetBundle, RouteAsset)

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

`taida build <PATH> --plan web-release` で `serverX` と `frontendA` を一括
ビルドします。CLI フラグの詳細は
[`docs/reference/cli.md::taida build`](../reference/cli.md) を参照してください。

---

## 9. バックエンド対応

`taida-lang/build` のディスクリプタ自体は **ビルドドライバ専用** であり、
ランタイムバックエンドの分類とは独立しています。`BuildUnit.target` で指定する
ターゲット (`native` / `js` / `wasm-*`) が、生成成果物のバックエンドを決めます。

| 関数 / 型 | Interpreter | Native | JS | WASM |
|-----------|-------------|--------|----|------|
| ディスクリプタ式の評価 | 受理 | 受理 | 受理 | 受理 |
| ランタイム値としての使用 | reject (ディスクリプタのみ) | 同左 | 同左 | 同左 |
