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
| `E17xx` | モジュール境界エラー | TypeChecker | `packages.tdm` 公開 API 不整合 |
| `E20xx` | アドオンマニフェストエラー | Addon manifest parser | `targets` 互換契約違反、未知ターゲット |

## 現在の割り当て

### 制約違反 (`E03xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E0301` | 単一方向制約違反 — `=>` と `<=` の混在禁止 | Parser / Verify |
| `E0302` | 単一方向制約違反 — `]=>` と `<=[` の混在禁止 | Parser / Verify |
| `E0303` | 単一方向制約違反 — `<=` の右辺に複数行の `\| cond \|> body` 多アーム条件を書けない (C20-1 silent-bug 禁圧) | Parser |

#### `E0303` — `<=` 右辺の複数行多アーム条件は禁止

**フェーズ**: Parser (`src/parser/parser_expr.rs::parse_cond_branch`)

**契機**: silent-bug 禁圧。`name <= | cond |> A | _ |> B` を複数行に分けて書くと、parser が続きの top-level 文を greedy に arm body として吸収する穴があった (`taida check` は通り、module load で symbol が消える)。**`CondBranchContext::LetRhs`** を `<=` 束縛の rhs で設定し、continuation arm が別行に現れたら `[E0303]` を発射します。

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
| `E1407` | Mold / Inheritance header の arity・prefix・重複・親種別が不正 | TypeChecker |
| `E1408` | MoldInst の `[]` 引数が concrete header 型に一致しない | TypeChecker |
| `E1409` | MoldInst の `[]` 引数が constrained header 型に一致しない | TypeChecker |
| `E1410` | InheritanceDef の子フィールドが親の型と互換でない再定義 | TypeChecker |

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

### モジュール境界エラー (`E17xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E1701` | `packages.tdm` で宣言された公開 API とエントリモジュールの実シンボル群が不整合 (未公開 symbol import / 宣言済み symbol 欠如 / module 内シンボル未発見) | TypeChecker |

### アドオンマニフェストエラー (`E20xx`)

`native/addon.toml` の parser が発射する診断。詳細仕様は
`docs/reference/addon_manifest.md` を参照。

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E2001` | `targets` 配列のエントリが許可リスト（現在は `{"native"}`）に含まれない | Addon manifest parser |
| `E2002` | `targets = []` — 空配列は許容しない（key を省略するとデフォルト `["native"]` が適用される） | Addon manifest parser |

## 帯域ルール

### 帯域の分類と境界

診断コードは発生フェーズに基づいて3つの層に分かれる。

#### 1. 前段ゲート（`taida check` で検出、backend に到達しない）

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
| `E17xx` | TypeChecker | モジュール境界 (`packages.tdm`) 公開 API 不整合 |

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
- **`E16xx` の Parser / TypeChecker 共有**: `E1616` は Parser が cond-branch の arm body を検査する時点で発射される。`E1617` は TypeChecker と `emit_wasm_c` の 2 箇所で発射される (同じ不変条件の検査を異なる段で別側面から行う意図的共有)。`E1609` / `E1615` は将来拡張用に予約された欠番
- **`E05xx` / `E06xx` / `E07xx` / `E08xx` / `E09xx` はカテゴリ予約**: 現時点で具体的な `E####` コードは未割当。モジュール解決 / ランタイム / codegen / パッケージ / グラフ各段のエラーは将来この帯域から採番する

### フォーマットの統一

現在、エラーメッセージ内のコード表記に2つの形式が混在している:

| 形式 | 使用箇所 | 例 |
|------|---------|-----|
| `E0301:` (コロン区切り) | Parser / Verify | `E0301: 単一方向制約違反 — ...` |
| `[E1301]` (ブラケット囲み) | TypeChecker | `[E1301] Function '...' takes at most ...` |

**正規形式**: `[E####]`（ブラケット囲み）を正規とする。`E03xx` のコロン形式は後方互換のため維持するが、新規コードは必ずブラケット形式を使用すること。`split_diag_code_and_hint` 関数は両形式を解析できる。

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

- `taida check --json` / `--diag-format jsonl` の出力に `E####` コードが含まれる
- AI ツールはコードを安定識別子として利用できる（文面に依存しない）
- 新規コード追加時はこのドキュメントを更新する
