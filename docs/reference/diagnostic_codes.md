# 診断コード体系 — `E####`

## 番号帯

| 帯域 | カテゴリ | フェーズ | 例 |
|------|---------|---------|-----|
| `E01xx` | 字句解析エラー | Lexer | 不正なトークン、未閉じ文字列 |
| `E02xx` | 構文解析エラー | Parser | 予期しないトークン、構文不正 |
| `E03xx` | 制約違反 | Parser / Verify | 単一方向制約、演算子混在禁止 |
| `E04xx` | 型エラー | TypeChecker | 型不一致、空リスト型推論不能 |
| `E05xx` | モジュールエラー | Module resolution | モジュール未発見、循環インポート |
| `E06xx` | ランタイムエラー | Interpreter | 未定義変数、ゼロ除算（Lax化済み） |
| `E07xx` | コード生成エラー | JS / Native | トランスパイル不能、未対応構文 |
| `E08xx` | パッケージエラー | Package manager | バージョン解決失敗、依存衝突 |
| `E09xx` | グラフエラー | Graph model | verify 失敗、構造不整合 |
| `E13xx` | 関数呼び出しエラー | TypeChecker | 引数過多、arity 不一致 |
| `E14xx` | モールド束縛エラー | TypeChecker | 必須引数不足、重複オプション、未定義フィールド |
| `E15xx` | 定義・意味論エラー | TypeChecker | 重複定義、禁止構文の明示拒否 |
| `E16xx` | 型推論・演算意味論エラー | TypeChecker | 戻り型不一致、列挙型不整合、演算子型不整合 |
| `E17xx` | CLI / モジュール境界エラー | CLI / TypeChecker | 削除済み CLI surface、`packages.tdm` 公開 API 不整合 |
| `E18xx` | 命名規則違反 (`taida way lint`) | Parser / Lint | カテゴリ別命名規則違反 |
| `E19xx` | ビルドドライバ系エラー | CLI / TypeChecker / Build driver | ディスクリプタビルドの文法、`AssetBundle` の安全性、`.taida/build` のトランザクショナル更新、依存閉包と成果物グラフの違反、内部フィールドへのアクセス禁止 |
| `E20xx` | アドオンマニフェストエラー | Addon manifest parser | `targets` 互換契約違反、未知ターゲット |
| `E32K1_*` | 自己アップグレード供給網エラー | `taida upgrade` | SHA-256 検証 / cosign 署名検証 / artifact 取得失敗 |
| `E32K2_*` | ロックファイル整合性エラー | `taida ingot` / `pkg::lockfile` | `taida.lock` schema バージョン / integrity 不一致 / migration 失敗 |
| `E32K3_*` | ソースパッケージ整合性エラー | `pkg::store` / `pkg::manifest` / `pkg::provider` | ソース pin / cosign 検証 / sha256 sidecar / 公式 namespace 制約 |

## 現在の割り当て

### 制約違反 (`E03xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E0301` | 単一方向制約違反 — `=>` と `<=` の混在禁止 | Parser / Verify |
| `E0302` | 単一方向制約違反 — `]=>` と `<=[` の混在禁止 | Parser / Verify |
| `E0303` | 単一方向制約違反 — `<=` の右辺に複数行の `\| cond \|> body` 多アーム条件を書けない (C20-1 silent-bug 禁圧) | Parser |

#### `E0303` — `<=` 右辺の複数行多アーム条件は禁止

**フェーズ**: Parser (`src/parser/parser_expr.rs::parse_cond_branch`)

**契機**: silent-bug 禁圧。`name <= | cond |> A | _ |> B` を複数行に分けて書くと、parser が続きの top-level 文を greedy に arm body として吸収する穴があった (`taida way check` は通り、module load で symbol が消える)。**`CondBranchContext::LetRhs`** を `<=` 束縛の rhs で設定し、continuation arm が別行に現れたら `[E0303]` を発射します。

**代替手段**:

1. `name <= If[cond, then, else]()` — 二肢条件の素直な表現
2. ヘルパ関数抽出 — `pickName ctx = | ... |> ... | _ |> ...`
3. 丸括弧でラップ — `name <= (| ... |> ... | _ |> ...)` (括弧が `CondBranchContext` を `TopLevel` に戻すため、多行形式でも境界が一意になる)

**許容される形**: single-line (すべての `|` が同じ物理行にある `name <= | a |> 1 | _ |> 2`) / top-level / function body / 括弧包み。


### 型エラー (`E04xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E0401` | リスト要素の型が不一致: 先頭要素は {T1} ですが、位置 {N} の要素は {T2} です | TypeChecker |

### 関数呼び出しエラー (`E13xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E1301` | 関数の引数が宣言されたパラメータ数を超過している | TypeChecker |
| `E1302` | デフォルト値式が自身または後続パラメータを参照している | TypeChecker |
| `E1303` | デフォルト値の型がパラメータの型注釈と不一致 | TypeChecker |

### モールド束縛エラー (`E14xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E1400` | TypeDef/Mold/Inheritance フィールドに型注釈または default がない | TypeChecker |
| `E1401` | MoldDef の追加 header 引数に束縛先がない | TypeChecker |
| `E1402` | MoldInst の `[]` 必須引数が不足 | TypeChecker |
| `E1403` | MoldInst の `[]` 引数が宣言数を超過 | TypeChecker |
| `E1404` | MoldInst の `()` オプションに同一名が重複 | TypeChecker |
| `E1405` | 必須フィールドが `()` オプション側に渡されている | TypeChecker |
| `E1406` | MoldInst の `()` に未定義のオプションが渡されている | TypeChecker |
| `E1407` | 親型適用の arity mismatch (header arity / prefix preservation / 親種別 / type param uniqueness を含む umbrella) | TypeChecker |
| `E1408` | MoldInst の `[]` 引数が concrete header 型に一致しない | TypeChecker |
| `E1409` | MoldInst の `[]` 引数が constrained header 型に一致しない | TypeChecker |
| `E1410` | declare-only な関数フィールドに既定関数または明示値が必要 (戻り型が defaultFn 生成不能な opaque / unknown alias の場合に定義位置で発火) | TypeChecker |
| `E1411` | 継承定義の子フィールドが親の型と互換でない再定義 | TypeChecker |
| `E1412` | `RustAddon["fn"](arity <= N)` の explicit binding 違反: 表記不正 (`fn` が文字列リテラルでない / `arity` field 欠落 / 非整数 arity) / facade コンテキスト外 / 未宣言の関数 / マニフェストとの arity 不一致 | Interpreter / TypeChecker |
| `E1413` | addon facade でマニフェスト `[functions]` の関数名を **bare 参照** している。`name <= RustAddon["name"](arity <= N)` を facade 先頭に明示する必要がある。`@e.X` 以降は移行コマンドを提供しないため、該当 facade は手動で修正する必要がある | Interpreter |

### 定義・意味論エラー (`E15xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E1501` | 同一スコープでの名前の再定義・関数オーバーロード禁止 | TypeChecker |
| `E1502` | 旧 `_` 部分適用構文の使用禁止 — 空スロット `f(5, )` を使うこと | TypeChecker |
| `E1503` | TypeDef/BuchiPack インスタンス化での部分適用禁止 | TypeChecker |
| `E1504` | パイプライン外での `Mold[_]()` 直接束縛禁止 | TypeChecker |
| `E1505` | 部分適用のスロット数が arity と不一致 | TypeChecker |
| `E1506` | 関数引数の型が宣言されたパラメータ型と不一致 | TypeChecker |
| `E1507` | ビルトイン関数の引数個数が arity 範囲外 | TypeChecker |
| `E1508` | メソッド呼び出しの引数個数または型が不一致 | TypeChecker |
| `E1509` | generic function の型変数が declared constraint を満たさない | TypeChecker |
| `E1510` | inference-only generic function の型変数が parameter annotation / call から束縛・推論できない、または concrete type 名と衝突する | TypeChecker |
| `E1511` | ユーザー定義関数を mold 構文 `Fn[args]()` で呼ぶ際に named fields `()` を渡せない — `Fn[a, b]()` か `Fn(a, b)` のみ | TypeChecker |

### 型推論・演算意味論エラー (`E16xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E1601` | 関数 / エラーハンドラの戻り型が宣言と不一致 | TypeChecker |
| `E1602` | BuchiPack / TypeDef の参照先にフィールドが存在しない | TypeChecker |
| `E1603` | `If` / cond-branch の then / else の戻り型が不一致 | TypeChecker |
| `E1604` | cond-branch の条件式が `Bool` 型ではない | TypeChecker |
| `E1605` | 比較演算 (`<` / `<=` / `>` / `>=` / `==` / `!=`) のオペランド型が不整合 | TypeChecker |
| `E1606` | 論理演算 (`&&` / `\|\|`) のオペランド型が `Bool` ではない | TypeChecker |
| `E1607` | 単項演算 (`-` / `!`) のオペランド型が不整合 | TypeChecker |
| `E1608` | 未定義の列挙型 / 列挙 variant が参照された | TypeChecker |
| `E1609` | (予約) | — |
| `E1610` | 継承関係 (`Inheritance`) に循環検出 | TypeChecker |
| `E1611` | JS バックエンドが受け付けない API capability (例: `httpServe(..., tls <= @(..., protocol <= Http2()))`) | TypeChecker |
| `E1612` | WASM バックエンドが受け付けない API capability (例: `taida-lang/net` の `httpServe`) | TypeChecker |
| `E1613` | `TypeExtends` が enum variant リテラルを受け付けない | TypeChecker |
| `E1614` | (tail-only mutual recursion detection guard — 発火は negative 形で検査、ハンドラ経路の保険コード) | TypeChecker |
| `E1615` | (予約) | — |
| `E1616` | cond-branch の arm body で bare call-statement (副作用のみの式) を禁止 | Parser |
| `E1617` | Regex invariant 違反 (wasm profile での `Regex` 参照、`__`-prefix field の衝突など) | TypeChecker / emit_wasm_c |
| `E1618` | モジュール境界越しの enum variant 並び順不一致 | TypeChecker |

#### `E1605` — 比較オペランド型の不整合

`E1605` は比較演算子そのものに対する前段ゲートであり、式ツリーの途中に埋まっていても発火します。対象には `stdout(...)` の引数、ユーザー関数 / メソッド呼び出しの引数、template interpolation (`${...}`)、BuchiPack / TypeInst のフィールド値、lambda body、cond arm body が含まれます。

この診断が出る program は Interpreter / JS / Native / WASM の各 backend に lowering されません。Enum や class-like value を順序比較したい場合は、先に明示的な数値化 API (例: `Ordinal[<enum>]()` など) を使って比較対象の型を揃えてください。

### CLI / モジュール境界エラー (`E17xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E1700` | E31 で削除された top-level command / CLI flag が呼ばれた。新しい command path / positional syntax を使うこと | CLI |
| `E1701` | `packages.tdm` で宣言された公開 API とエントリモジュールの実シンボル群が不整合 (未公開 symbol import / 宣言済み symbol 欠如 / module 内シンボル未発見) | TypeChecker |

`E1700` の標準表示:

```text
[E1700] Command '<old>' was removed in @e.X. Use '<replacement>' instead.
        See `taida --help` for the new command set.
```

削除済み flag の表示例:

```text
[E1700] Flag '--target <target>' was removed in @e.X. Use 'taida build <target> <PATH>' instead.
        For example: `taida build js src`.
```

### 命名規則違反 — `taida way lint` (`E18xx`)

カテゴリ別命名規則を CI で確認する lint 診断帯です。`taida way lint <PATH>` で実行します。

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E1801` | クラスライク型 / モールド型 / スキーマ / エラー variant は PascalCase で命名してください | Parser / Lint |
| `E1802` | 関数は camelCase で命名してください | Parser / Lint |
| `E1803` | 関数値を束縛する変数は camelCase で命名してください | Parser / Lint |
| `E1804` | 非関数値を束縛する変数は snake_case で命名してください | Parser / Lint |
| `E1805` | (予約) 定数は SCREAMING_SNAKE_CASE で命名してください。Taida は構文上「定数」を変数と区別しないため、AST 単独走査では検出できません。利用情報を扱う後段への接続を予約しています | Parser / Lint |
| `E1806` | エラー variant / 列挙 variant は PascalCase で命名してください | Parser / Lint |
| `E1807` | 型変数は単一大文字 (`T`, `U`, `V`, `E`, `K`, `P`, `R` など) で命名してください (4 つ以上の場合に `T1` / `T2` / `T3` といった添字形は許容します) | Parser / Lint |
| `E1808` | ぶちパックフィールドの値型と命名規則が不整合 (関数値は camelCase、非関数値は snake_case) | Parser / Lint |
| `E1809` | 戻り値型注釈には `:Type` の `:` マーカーを付けてください (例: `=> :Int`)。parser は寛容に受理しますが、戻り値型・制約・型引数のスロット・`TypeIs`・モールドの具象型スロットなど、型リテラルが必要な文脈では必須です | Parser / Lint |

#### 適用範囲 / 適用対象外

- 適用対象外: `_` プレフィックス (`_internal` など)、boolean プレフィックス (`is` / `has` / `can` / `did` / `needs`)、引数・フィールド型注釈の形式 A (`arg: Type`) と形式 B (`arg :Type`) の選択
- `E1805` は AST 単独走査で定数を検出できないため予約 (将来拡張)
- `E1809` は parser が寛容に `=> Type` を受理した場合に lint で検出します。CI ではエラー扱いです

### ビルドドライバ系エラー (`E19xx` 予約)

複数バックエンド混合ビルド (`docs/reference/build_descriptors.md`) で発射する診断帯です。

| 区切り | 用途 | 発射段 |
|--------|------|--------|
| `E1900〜E1909` | ディスクリプタビルドの CLI 文法と曖昧さ排除 — 上限 `E1909` は (予約) | CLI |
| `E1910〜E1919` | `AssetBundle` のパス / グロブ / シンボリックリンク / ディスクリプタ名安全性 — 上限 `E1919` は (予約) | Build driver |
| `E1920〜E1929` | `.taida/build` のトランザクショナル更新 / ステージング掃除 / アトミック置換非対応 — `E1920` / `E1929` は (予約) | Build driver |
| `E1930〜E1939` | 多成果物診断スキーマの予約・移行用 — 帯全体 (予約) | Build driver / Diagnostics |
| `E1940〜E1949` | 成果物グラフの循環 / ターゲット依存閉包違反 — 上限 `E1949` は (予約) | Build driver / TypeChecker |
| `E1950〜E1959` | `BuildHook` の検証 / 実行失敗 — 上限 `E1959` は (予約) | Build driver |
| `E1960` | 内部 `__` フィールドへのユーザ向けドットアクセス禁止 | TypeChecker / Runtime |

ビルドドライバ由来の診断はこの `E19xx` 帯から採番します。jsonl レコードへの `build` ブロック付与ルールと、テキスト出力での `unit=...` / `target=...` / `edge=... dependency=...` 行の扱いは `docs/reference/build_descriptors.md` の 9 節を参照してください。

| コード | 概要 |
|--------|------|
| `E1900` | ディスクリプタビルドと単一ターゲットビルドの曖昧な CLI 組み合わせ |
| `E1901` | `--unit` / `--plan` / `--all-units` の複数指定 |
| `E1902` | ディスクリプタ入力・フィールド形状・export 不在、または `BuildUnit` / `BuildPlan` / `AssetBundle` / `BuildHook` の `name` / シンボル重複 |
| `E1903` | 指定 `BuildUnit` 不在 |
| `E1904` | 指定 `BuildPlan` 不在 |
| `E1910` | `AssetBundle.root` / 参照 asset の検証失敗 |
| `E1911` | `AssetBundle.files` glob の検証失敗 |
| `E1912` | AssetBundle ソース正規化・コピー失敗 |
| `E1913` | AssetBundle が symlink / 非通常ファイルを含む |
| `E1914` | AssetBundle 出力パスの検証失敗または重複 |
| `E1915` | `RouteAsset.path` の形式不正または重複 |
| `E1916` | `BuildUnit` / `BuildPlan` / `AssetBundle` / `BuildHook` の `name` が単一パスセグメント制約を満たさない (空 / `..` / `/` / `\\` / 先頭ドット / NUL) |
| `E1922` | stale staging cleanup 失敗 |
| `E1923` | staging / child build / artifact-map 作成失敗 |
| `E1924` | atomic replace / rollback 失敗 |
| `E1940` | 成果物依存サイクル |
| `E1941` | descriptor import / target closure 検証失敗 |
| `E1942` | 子 `BuildUnit` のターゲットビルド失敗 |
| `E1950` | `BuildHook` 参照または cwd 検証失敗 |
| `E1951` | `BuildHook` が付与されているが `--run-hooks` が無い |
| `E1952` | `BuildHook` 実行またはログ書き込み失敗 |

### 帯域を再利用する予約コード

新帯域を切らず、既存帯域を再利用して採番する予約コードです。

| コード (予約) | 内容 | 既存帯域 |
|--------------|------|---------|
| `E0700` | Native と native lowering 系 WASM で相互再帰を検出した場合の拒否 | `E07xx` コード生成エラー |
| `E1508` | `Lax[T].getOrDefault` / `getOrThrow` / `map` / `flatMap` の引数型不整合 | `E15xx` 定義・意味論エラー |

### アドオンマニフェストエラー (`E20xx`)

`native/addon.toml` の parser が発射する診断。詳細仕様は
`docs/reference/addon_manifest.md` を参照。

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E2001` | `targets` 配列のエントリが許可リスト（現在は `{"native"}`）に含まれない | Addon manifest parser |
| `E2002` | `targets = []` — 空配列は許容しない（key を省略するとデフォルト `["native"]` が適用される） | Addon manifest parser |

### 自己アップグレード供給網エラー (`E32K1_*`)

`taida upgrade` が GitHub Releases から SHA256SUMS および artifact を取得し、cosign で
verify する経路で発射する診断。`@e.32` beta で `--required` 既定。

| コード | 発生条件 | 推奨対応 |
|--------|----------|---------|
| `E32K1_UPGRADE_DOWNLOAD_FAILED` | release artifact / SHA256SUMS / cosign bundle の取得 (HTTP / `file://`) が失敗、または HTTP non-2xx | ネットワーク状態を確認し再実行。proxy / firewall がある場合は `https://api.github.com` および `https://github.com/taida-lang/taida/releases/...` への到達性を確認 |
| `E32K1_UPGRADE_STAGE_FAILED` | 取得済み bytes を `/tmp/taida_upgrade_<pid>_<nanos>_*` に書き出す段階で失敗 | `/tmp` の disk 空き容量 / 権限を確認。`TMPDIR` を任意のディレクトリへ変更可 |
| `E32K1_UPGRADE_NO_SHA256SUMS` | release に SHA256SUMS が公開されていない、もしくは対象 archive 行が欠落 | release tag を再確認し、リリースワークフローが SHA256SUMS を上げ直すまで待機 |
| `E32K1_UPGRADE_SHA256_MISMATCH` | 取得 archive の SHA-256 が SHA256SUMS の expected と一致しない | 取得を再試行。`--force-refresh` 等で cache を無効化、それでも mismatch が続く場合は供給網侵害を疑い、手元 binary に切り戻して `taida-lang/taida` security advisory を確認 |
| `E32K1_UPGRADE_SHA256SUMS_INVALID_ENCODING` | cosign 検証通過後の SHA256SUMS が UTF-8 として decode できない | 上記 `_SHA256_MISMATCH` と同様に供給網侵害を疑う。release を再公開する判断は upstream 側 |
| `E32K1_COSIGN_MISSING` | `taida upgrade` 実行環境の `PATH` 上に `cosign` が存在しない | `cosign` (sigstore) を install して `PATH` に通す。`@e.32` beta では `taida upgrade` の cosign verify は **必須** |
| `E32K1_UPGRADE_SHA256SUMS_COSIGN_MISSING` | `SHA256SUMS.bundle` が release から取得できない | release ワークフローが bundle を上げ直すまで待機 |
| `E32K1_UPGRADE_SHA256SUMS_COSIGN_REJECTED` | cosign が SHA256SUMS の署名を拒否 (identity / certificate-identity-regexp 不一致など) | 公式 release tag であることを確認。手動再取得後も再現する場合は供給網侵害として upstream に報告 |
| `E32K1_UPGRADE_SHA256SUMS_COSIGN_ERROR` | cosign の起動 / 内部エラーで verify が完了しない | `cosign version` を確認し、最新版に更新。`/tmp` の権限不足や AppArmor / SELinux 等の sandbox 制約が原因のことがある |

### ロックファイル整合性エラー (`E32K2_*`)

`taida.lock` (schema v2 = `sha256:` + 64 hex 必須) の load / migrate / drift 検査が発射する診断。

| コード | 発生条件 | 推奨対応 |
|--------|----------|---------|
| `E32K2_LOCKFILE_V1_REJECTED` | schema v1 (`fnv1a` 等の legacy integrity) の `taida.lock` を load しようとした | `taida ingot migrate-lockfile` で schema v2 に移行。lockfile を直接書き換えない |
| `E32K2_LOCKFILE_UNSUPPORTED_VERSION` | `taida.lock` schema が現在の taida binary がサポートする `LOCKFILE_SCHEMA_VERSION` より新しい | taida binary を `taida upgrade` で更新するか、互換 lockfile を生成し直す |
| `E32K2_LOCKFILE_INTEGRITY_MISMATCH` | `validate_resolved_bindings` が `--frozen` 非依存で発射する 4 経路: (1) `taida.lock` の `integrity` が `sha256:` 以外の prefix (legacy / unknown algorithm)、(2) resolved package が `taida.lock` から欠落、(3) `(version, source, integrity)` triple が resolver 結果と一致しない、(4) `taida.lock` の package 件数と resolver 件数が不一致 | `taida ingot update` で再解決 (`--frozen` を外す)、または上流の package version pin を見直す。`sha256:` 以外の prefix は `taida ingot migrate-lockfile` 経由で v2 に正規化 |
| `E32K2_LOCKFILE_DRIFT` | `--frozen` 指定で `.taida/taida.lock` が欠落、または `is_up_to_date` 検査で `packages.tdm` と drift | `taida ingot update` で lockfile を再生成して commit。CI で `--frozen` を保つ場合は事前に開発者側で lock を更新 |
| `E32K2_LOCKFILE_MIGRATE_FAIL` | `taida ingot migrate-lockfile` で installed dependency が見つからない、SHA-256 計算に失敗 | `.taida/deps/...` の中身を確認、`taida ingot install` で取得し直す |
| `E32K2_INTEGRITY_UNSUPPORTED_ENTRY` | tarball / extracted dir に non-regular file (symlink / device / fifo 等) が含まれ SHA-256 stream walker が traverse できない | 当該パッケージの公式 archive を確認。手元 fork の場合は構造をフラットなファイル構成へ修正 |

### ソースパッケージ整合性エラー (`E32K3_*`)

ソースパッケージ (`taida-lang/*` の `[packages."x"]`) が cosign + SHA-256 で
verify されることを保証する診断。`@e.32` beta では
`taida-lang/*` 以外のソース pin は受理しない。

| コード | 発生条件 | 推奨対応 |
|--------|----------|---------|
| `E32K3_GITHUB_BASE_URL_CONFINED` | source package install 時に `TAIDA_GITHUB_BASE_URL` が `https://github.com` 以外を指している | `@e.32` beta で source archive は `https://github.com` 固定。テスト環境で override する場合は `cargo test` 内の専用 helper を使う |
| `E32K3_NON_OFFICIAL_SOURCE_REJECTED` | `[packages."x"]` table に `taida-lang/*` 以外の owner の source pin を書いた | `@e.32` beta では公式 namespace のみ。3rd party は addon (cdylib) として配布する設計 |
| `E32K3_SOURCE_INTEGRITY_MISSING` | `[packages."x"]` table に `integrity` が無い、または `sha256:<64 lowercase hex>` 以外の prefix | `packages.tdm` に `integrity = "sha256:<64 hex>"` を追加。値は upstream release の sidecar から取得 |
| `E32K3_SOURCE_INTEGRITY_MISMATCH` | キャッシュ済み source archive / cached store の SHA-256 が `packages.tdm` の expected と一致しない | `taida ingot install --force-refresh` で再取得。再現する場合は供給網侵害を疑う |
| `E32K3_SOURCE_INTEGRITY_UNVERIFIED` | キャッシュ済み source archive に SHA-256 sidecar が無い、または読めない | `taida ingot install --force-refresh` で sidecar を再生成 |
| `E32K3_SOURCE_COSIGN_REQUIRED` | source archive の cosign 検証が `Required` policy 下で skip / warn / 失敗 | `cosign` を install し、release が公式 cosign-signed であることを確認。`TAIDA_VERIFY_SIGNATURES` を緩めない |
| `E32K3_VERIFY_SIGNATURES_RELAXED` | source package install 時に `TAIDA_VERIFY_SIGNATURES` が `required` 以外 (もしくは未設定の cosign 不在) | install を再実行し、`required` を強制する。`@e.32` beta で source archive は `Required` 必須 |
| `E32K3_PACKAGES_TDM_DUPLICATE_TABLE` | 同一 `[packages."<id>"]` ブロックが `packages.tdm` に複数存在する。後続テーブルで pin が silent に上書きされてレビューで気付けない状態 | 重複ブロックを 1 つにまとめ、source pin が一意になる形に書き直す |

## 帯域ルール

### 帯域の分類と境界

診断コードは発生フェーズに基づいて3つの層に分かれる。

#### 1. 前段ゲート（`taida way check` / `taida way` で検出、backend に到達しない）

| 帯域 | フェーズ | 責務 |
|------|---------|------|
| `E01xx` | Lexer | 字句解析。不正なトークン、未閉じ文字列 |
| `E02xx` | Parser | 構文解析。予期しないトークン、構文不正 |
| `E03xx` | Parser / Verify | 構造的制約。単一方向制約違反 |
| `E04xx` | TypeChecker | 型不一致、型推論不能 |
| `E05xx` | Module resolution | モジュール未発見、循環インポート |
| `E13xx` | TypeChecker | 関数呼び出し意味論。arity 不一致、デフォルト値制約 |
| `E14xx` | TypeChecker | モールド束縛意味論。フィールド重複、引数不足 |
| `E15xx` | TypeChecker | 定義・意味論。重複定義、禁止構文の明示拒否 |
| `E16xx` | TypeChecker / Parser | 型推論・演算意味論。戻り型、比較、論理、cond-branch、循環継承 |
| `E17xx` | CLI / TypeChecker | 削除済み CLI surface / モジュール境界 (`packages.tdm`) 公開 API 不整合 |
| `E18xx` | Parser / Lint | 命名規則違反 (`taida way lint` 実行時のみ発射) |
| `E19xx` | CLI / TypeChecker / Build driver | ビルドドライバ系 (ディスクリプタビルド文法、`AssetBundle` 安全性、トランザクショナル更新、依存閉包違反、内部フィールドアクセス禁止) |

#### 2. Backend 層（コード生成時に検出）

| 帯域 | フェーズ | 責務 |
|------|---------|------|
| `E07xx` | JS / Native codegen | トランスパイル不能、未対応構文 |

#### 3. 実行時層

| 帯域 | フェーズ | 責務 |
|------|---------|------|
| `E06xx` | Interpreter | ランタイムエラー（未定義変数等。Lax化により縮小傾向） |

#### 4. 周辺ツール層

| 帯域 | フェーズ | 責務 |
|------|---------|------|
| `E08xx` | Package manager | バージョン解決失敗、依存衝突 |
| `E09xx` | Graph model | verify 失敗、構造不整合 |

### 帯域重複の排除

- **前段ゲート内の重複なし**: `E01xx`-`E05xx` は入力処理の順序に沿った連番。`E13xx`-`E15xx` は TypeChecker 内の意味分類。両者は帯域が重複しない
- **`E03xx` の Parser / Verify 共有**: `E0301`/`E0302` は Parser と Verify の両方で検出される。これは同一の制約違反を2箇所で検出するための意図的な共有であり、帯域重複ではない
- **`E10xx`-`E12xx` は予約**: 将来の TypeChecker 拡張用に確保。現在は未使用
- **`E18xx` は lint 帯域**: `taida way lint` 実行時のみ発射し、`taida way check` / `taida build` の前段ゲートには含めない (lint と check は別レイヤー、CI では別 job)
- **`E16xx` の Parser / TypeChecker 共有**: `E1616` は Parser が cond-branch の arm body を検査する時点で発射される。`E1617` は TypeChecker と `emit_wasm_c` の 2 箇所で発射される (同じ不変条件の検査を異なる段で別側面から行う意図的共有)。`E1609` / `E1615` は将来拡張用に予約された欠番
- **`E05xx` / `E06xx` / `E07xx` / `E08xx` / `E09xx` はカテゴリ予約**: 現時点で具体的な `E####` コードは未割当。モジュール解決 / ランタイム / codegen / パッケージ / グラフ各段のエラーは将来この帯域から採番する

### フォーマットの統一

現在、エラーメッセージ内のコード表記に2つの形式が混在している:

| 形式 | 使用箇所 | 例 |
|------|---------|-----|
| `E0301:` (コロン区切り) | Parser legacy | `E0301: 単一方向制約違反 — ...` |
| `[E1301]` (ブラケット囲み) | TypeChecker | `[E1301] Function '...' takes at most ...` |

**正規形式**: `[E####]`（ブラケット囲み）を正規とする。`E03xx` のコロン形式は後方互換のため解析可能なまま維持するが、新規コードは必ずブラケット形式を使用すること。構造化診断の code 抽出は `src/diagnostics.rs::split_diag_code_and_hint` を source of truth とし、CLI / verify の JSONL 出力は両形式を同じ `code` として扱う。

## 命名規約

1. **形式**: `E` + 4桁ゼロ埋め数字（`E0001` 〜 `E9999`）
2. **帯域内の番号**: 先頭2桁がカテゴリ、末尾2桁が連番（`01` から開始）
3. **新規追加**: 各帯域の末尾に追番する。欠番を埋めない
4. **メッセージ言語**: エラーメッセージ本文は日本語（`--diag-format jsonl` 出力時も同様）
5. **コード表記**: 新規コードは `[E####]` ブラケット形式を使用する

## 後方互換ポリシー

### 不変

- **コードの意味は変更しない**: 一度割り当てた `E####` の意味（どの種類のエラーか）は変更しない
- **コードの再利用禁止**: 廃止したコードを別の意味で再利用しない

### 許容

- **メッセージ文面の改善**: 同一コードのエラーメッセージ文面は改善のため変更できる
- **コードの廃止**: 言語仕様の変更により不要になったコードは廃止できる（帯域は欠番になる）
- **severity の変更**: error → warning、warning → error の変更は可能（ただしリリースノートに明記）

### AI 連携

- `taida way check --format json` / `taida way verify --format jsonl` / `taida build --diag-format jsonl` の出力に `E####` コードが含まれる
- AI ツールはコードを安定識別子として利用できる（文面に依存しない）
- 新規コード追加時はこのドキュメントを更新する
