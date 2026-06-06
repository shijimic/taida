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
| `E07xx` | コード生成エラー | Native / WASM / legacy JS | 生成不能、未対応構文 |
| `E08xx` | パッケージエラー | Package manager | バージョン解決失敗、依存衝突 |
| `E09xx` | グラフエラー | Graph model | verify 失敗、構造不整合 |
| `E13xx` | 関数呼び出しエラー | TypeChecker | 引数過多、アリティ不一致 |
| `E14xx` | モールド束縛エラー | TypeChecker | 必須引数不足、重複オプション、未定義フィールド |
| `E15xx` | 定義・意味論エラー | TypeChecker | 重複定義、禁止構文の明示拒否 |
| `E16xx` | 型推論・演算意味論エラー | TypeChecker | 戻り型不一致、列挙型不整合、演算子型不整合 |
| `E17xx` | CLI / モジュール境界エラー | CLI / TypeChecker | 削除済み CLI surface、`packages.tdm` 公開 API 不整合 |
| `E18xx` | 命名規則違反 (`taida way lint`) | Parser / Lint | カテゴリ別命名規則違反 |
| `E19xx` | ビルドドライバ系エラー | CLI / TypeChecker / Build driver | ディスクリプタビルドの文法、`AssetBundle` の安全性、`.taida/build` のトランザクショナル更新、依存閉包と成果物グラフの違反、内部フィールドへのアクセス禁止 |
| `E20xx` | アドオンマニフェストエラー | アドオンマニフェストパーサー | `targets` 互換契約違反、未知ターゲット |
| `E36xx` | host boundary 型制約エラー | TypeChecker | `Wired[T]`、host step list の境界違反 |
| `E32K1_*` | 自己アップグレード供給網エラー | `taida upgrade` | SHA-256 検証 / cosign 署名検証 / artifact 取得失敗 |
| `E32K2_*` | ロックファイル整合性エラー | `taida ingot` / `pkg::lockfile` | `taida.lock` schema バージョン / integrity 不一致 / migration 失敗 |
| `E32K3_*` | ソースパッケージ整合性エラー | `pkg::store` / `pkg::manifest` / `pkg::provider` | ソース pin / cosign 検証 / sha256 sidecar / 公式 namespace 制約 |
| `E32K4_*` | パッケージ facade 整合性エラー | `taida ingot publish` / module import | `packages.tdm` facade と entry module export surface の不一致 |

## コード一覧

### 制約違反 (`E03xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E0301` | 単一方向制約違反 — `=>` と `<=` の混在禁止 | Parser / Verify |
| `E0302` | 単一方向制約違反 — `>=>` と `<=<` の混在禁止 | Parser / Verify |
| `E0303` | 単一方向制約違反 — `<=` の右辺に複数行の `\| cond \|> body` 多アーム条件を書けない | パーサー |
| `E0304` | `<=` チェーン (逆方向パイプライン) のステップが代入対象と同一物理行に収まっていない | Parser |

#### `E0304` — `<=` チェーンのステップは単一物理行に収まる必要がある

**フェーズ**: Parser

**契機**: `<=` チェーン (`result <= f(_) <= g(_) <= data`) は逆方向パイプラインを成立させるための「**単一文・単一物理行**」の構文です。代入対象 (`result`) と全てのステップ・データが同じ物理行に置かれる必要があります。改行を跨いだチェーンの吸収を許すと、後続の独立した文をチェーンの一部として気付かれずに取り込んでしまう穴が生じるため、ステップの parse 範囲が代入対象の行を逸れた場合は `[E0304]` を発射します。

**典型的な reject 形**:

- ステップに改行を含む呼び出し引数:
  ```taida reject
  result <= double(_) <= addThree(
    1,
    2,
    3
  )
  ```
- 括弧で包まれた multi-line 式を first rhs に置く:
  ```taida reject
  result <= (
    1 + 2
  ) <= 99
  ```
- backslash 継続:
  ```taida reject
  result <= plusOne(_) <= \
    4
  ```

**代替手段**:

- 中間変数で分割して書く。`data <= addThree(1, 2, 3)` を別文に切り出し、続けて `result <= double(_) <= data`。
- 順方向 `=>` パイプラインを使い、`data => addThree(1, 2, 3) => double(_) => result` のように 1 行で書く。
- 関数を抽出してパイプラインを短くする。

#### `E0303` — `<=` 右辺の複数行多アーム条件は禁止

**フェーズ**: Parser

**契機**: 気付かれずにバグが入り込むのを抑止するためのゲートです。`name <= | cond |> A | _ |> B` を複数行に分けて書くと、パーサーが続きのトップレベル文を貪欲にアーム本体として吸収する穴があります。`<=` 束縛の右辺で条件分岐の文脈を判別し、継続アームが別行に現れたら `[E0303]` を発射します。

**代替手段**:

1. `name <= If[cond, then, else]()` — 二肢条件の素直な表現
2. ヘルパ関数抽出 — `pickName ctx = | ... |> ... | _ |> ...`
3. 丸括弧でラップ — `name <= (| ... |> ... | _ |> ...)` (括弧が `CondBranchContext` を `TopLevel` に戻すため、多行形式でも境界が一意になる)

**許容される形**: 単一行 (すべての `|` が同じ物理行にある `name <= | a |> 1 | _ |> 2`) / トップレベル / 関数本体 / 括弧包み。


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
| `E1401` | モールド定義の追加ヘッダー引数に束縛先がない | TypeChecker |
| `E1402` | MoldInst の `[]` 必須引数が不足 | TypeChecker |
| `E1403` | MoldInst の `[]` 引数が宣言数を超過 | TypeChecker |
| `E1404` | MoldInst の `()` オプションに同一名が重複 | TypeChecker |
| `E1405` | 必須フィールドが `()` オプション側に渡されている | TypeChecker |
| `E1406` | MoldInst の `()` に未定義のオプションが渡されている | TypeChecker |
| `E1407` | 親型適用のアリティ不一致 (ヘッダーのアリティ、型引数の前置保存、親種別、型パラメータの一意性などをまとめて扱う統合診断) | TypeChecker |
| `E1408` | モールドインスタンスの `[]` 引数が、具体型として宣言されたヘッダーの型に一致しない | TypeChecker |
| `E1409` | モールドインスタンスの `[]` 引数が、制約付き型変数として宣言されたヘッダーの型に一致しない | TypeChecker |
| `E1410` | 宣言のみの関数フィールドに既定関数または明示値が必要。戻り型が不透明型や不明な型エイリアスで `defaultFn` を自動生成できない場合に、定義位置で発火する | TypeChecker |
| `E1411` | 継承定義の子フィールドが親の型と互換でない再定義 | TypeChecker |
| `E1412` | `RustAddon["fn"](arity <= N)` の明示束縛違反: 表記不正 (`fn` が文字列リテラルでない / `arity` フィールド欠落 / アリティが非整数) / ファサード以外で書いている / 未宣言の関数 / マニフェストのアリティと不一致 | Interpreter / TypeChecker |
| `E1413` | アドオンファサードでマニフェスト `[functions]` の関数名を **裸のまま (bare 参照)** している。`name <= RustAddon["name"](arity <= N)` をファサード先頭で明示する必要がある | Interpreter |

### 定義・意味論エラー (`E15xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E1501` | 同一スコープでの名前の再定義・関数オーバーロード禁止 | TypeChecker |
| `E1502` | 旧 `_` 部分適用構文の使用禁止 — 空スロット `f(5, )` を使うこと | TypeChecker |
| `E1503` | TypeDef/BuchiPack インスタンス化での部分適用禁止 | TypeChecker |
| `E1504` | パイプライン外での `Mold[_]()` 直接束縛禁止 | TypeChecker |
| `E1505` | 部分適用のスロット数がアリティと不一致 | TypeChecker |
| `E1506` | 関数呼び出しの引数型が宣言されたパラメータ型と不一致、または関数値引数の省略推論が解決不能 | TypeChecker |
| `E1507` | ビルトイン関数の引数個数がアリティの許容範囲外 | TypeChecker |
| `E1508` | 関数値を受け取るメソッドまたは関数呼び出しの引数個数・型・推論境界が不一致 (`Lax[T]` / `Result[T, P]` / `Async[T]` の `map` / `flatMap` / `mapError`、`List[T].fold` / `reduce` の関数引数型ピン違反を含む) | TypeChecker |
| `E1509` | メソッドが型に存在しない、またはジェネリック関数の型変数が宣言された制約を満たさない (`errorInfo()` を `Lax` / `Gorillax` / `RelaxedGorillax` / `Error` 系以外の型に呼んだ場合を含む) | TypeChecker |
| `E1510` | 推論専用ジェネリック関数の型変数が、パラメータの型注釈や呼び出し側から束縛・推論できない、または具体型名と衝突する | TypeChecker |
| `E1511` | ユーザー定義関数をモールド構文 `Fn[args]()` で呼ぶ際に名前付き引数 `()` を渡せない — `Fn[a, b]()` か `Fn(a, b)` のみ受理する | TypeChecker |
| `E1512` | `Cage` / `CageRilla`: コンパイル時のブランチ不一致 — `Cage[subject, runner]()` で `subject` 側の `Molten` ブランチと、`runner` (`CageRilla[Branch, Out]`) の `Branch` 型引数が一致しない | TypeChecker |
| `E1513` | (予約) `Cage` / `CageRilla`: 実行時のブランチ不一致 — 実行時のメタデータで `subject` 側と `runner` 側のブランチが一致しないことを検出 | Runtime |
| `E1514` | `Cage` / `CageRilla`: 古い記法 `Cage[molten, lambda]()` のような lambda 直接渡し形は受理しない。正規形である `Cage[subject, JSRilla[...]()]()` のように、`CageRilla` 系の実行記述経由で書き換える | TypeChecker |
| `E1515` | `Cage` / `CageRilla`: `JSGet` / `JSCall` / `JSCallAsync` / `JSNew` / `JSSet` / `JSBind` / `JSSpread` を `Cage` の外で JS 操作として直接書く記法は受理しない。`Cage[subject, JSCall[path, args, Out]()]()` のように `Cage` の実行記述として中に置く | TypeChecker |
| `E1516` | `Cage` / `CageRilla`: 子系統のアリティ違反 — `JSRilla[JS, Out]` / `BuildRilla[Build, Out]` / `FileRilla[File, Out]` のようにブランチ名を第 2 引数に書く形は採用しない。子系統は `JSRilla[Out]` / `BuildRilla[Out]` / `FileRilla[Out]` の `[Out]` 1 引数のみで書く | TypeChecker |
| `E1517` | `Cage` / `CageRilla`: 型検査終了時点で subject 側または runner 側のブランチが未解決のため、`Cage[target, runner]()` の境界規約を静的に証明できない | TypeChecker |
| `E1518` | `Cage` / `CageRilla`: `JSON[raw, Schema]()` のようなスキーマ変換ファサードを `Cage` の subject または runner に渡す境界規約違反。これらのファサードは失敗を `Lax[T]` で表現する経路を維持しており、`Cage` / `Gorillax` の経路には流さない | TypeChecker |
| `E1519` | `Cage` / `CageRilla`: JS runner (`JSCall` / `JSNew` / `JSCallAsync`) の `Out` に `Async[T]` を書く Promise 境界の混在は受理しない。Promise-returning JS call は `JSCallAsync[path, args, T]()` を使い、`Cage[subject, JSCallAsync[...]]()` の戻り値 `Async[T]` を `>=>` で待つ | TypeChecker |
| `E1520` | 「値の不在を表す型」の完全排除 — `@()` (空ぶちパック) を「型」として書くことを禁止: **(R1)** 戻り型注釈 `:@()` / `:Unit` / `:Void`、**(R1対称)** 引数型注釈、**(R2)** 関数本体末尾の `@()` / `Unit` / `Void` リテラル、**(R2拡張)** 注釈なしで最終推論型が `@()` 等に確定 (中間変数経由の抜け道も塞ぐ)、**(R3)** ClassLike / Mold / Inheritance 宣言の body が空 (`Pilot = @()` / `Int => PilotId = @()` / `Mold[T] => Box[T] = @()` 等) で、Parser が `[E1520]` を即時 reject、**(ジェネリック)** `Mold[T]` で `T = @()` 等に具象化、を `Parser` / `TypeChecker` で reject。Taida ではぶちパック値が動的に `@()` になるケースは構造的に発生しない (汎用 filter が存在せず、縮小操作は別の具体ぶちパック型を返すため)。情報がない場合は意味を持った値 (書き込んだバイト数、状態を表すぶちパック、共通 Enum のバリアント等) を返す。空リスト `@[]` は「空のリスト」として明確な意味を持つので別物。 | Parser / TypeChecker |
| `E1521` | ぶちパックリテラルの位置指定フィールド (`@(v1, v2, ...)`) は受理しない。PHILOSOPHY II「ぶちパック `@(...)` — 名前付きフィールドの集合」と矛盾するため、すべてのぶちパックフィールドは名前付き (`@(name <= value, ...)`) で書く必要がある。`<<<` / `>>>` の名前リスト (`@(name1, name2)`) や、型定義 / クラスライク型のフィールド定義 (`@(name: Type)`) は、別の文脈として引き続き受理する。 | Parser |
| `E1523` | モールド宣言ヘッダーの型変数名が、組み込み型名 (`Int` / `Str` / `Bool` / `Float` / `Bytes` / `Lax` / `Result` / `Async` / `Optional` / 等) と衝突。`Mold[Int]` は型変数名 `Int` として警告なく解釈されるが、書き手の意図は具体型 `Int` のほうが多い。曖昧さを避けるため `Mold[:Int]` (具体型直接指定) または `Mold[T <= :Int]` (制約付き型変数) を使う。 | TypeChecker |
| `E1524` | 条件分岐 `\| cond \|>` から既定アーム (`\| _ \|>` / `\| true \|>`) が欠落。`\| _ \|>` または `\| true \|>` を追加して、全入力で結果が定義されるようにする。PHILOSOPHY IV「AI が構造として読めるよう、parser が一意に解釈でき、docs と実装が同じ規約を持つ」。 | TypeChecker |
| `E1525` | 名前付き関数の引数や式のオペランドなど、公開境界に残った型変数を確定できない。戻り型・呼び出し文脈だけで決まらないパラメータや式には明示的な型注釈が必要。 | TypeChecker |
| `E1526` | 名前付き関数・メソッド定義に戻り型注釈 `=> :Type` がない。パーサーは定義構文の末尾で検出し、型チェッカーは構文木境界の最終確認として検出する。 | Parser / TypeChecker |
| `E1527` | lambda パラメータの型を推論できない。`_ x: Type = ...` と書くか、lambda を `Function([A], B)` が要求される位置で使う必要がある。 | TypeChecker |
| `E1528` | lambda パラメータの明示型注釈が、呼び出し元から要求される関数型のパラメータ型と一致しない。 | TypeChecker |
| `E1529` | 型検査完了後の内部表現に未解決型が残っている。未解決型を含むプログラムはバックエンドのコード生成段階に渡されない | TypeChecker |
| `E1530` | 未定義のモールド名を `Name[args]()` として呼び出した。既知のモールド / 型 / 関数名を定義してから使うか、通常の関数呼び出し `name(args)` を使う必要がある。 | TypeChecker |
| `E1531` | `.throw()` の対象が `Error` 系の値ではない。`Error` を継承した型の値を構築してから throw する必要がある。 | TypeChecker |

`E1512`〜`E1519` は **`Cage` / `CageRilla` 診断範囲**。`Cage[subject, runner]()` の型規則、および `CageRilla[Branch, Out]` の子系統 (`JSRilla` / `JSONRilla` / `BuildRilla` / `FileRilla`) が守る境界規約を扱う。`E1513` は将来の実行時検証用に予約している。

`E1525`〜`E1531` は **完全固定境界と式 surface の診断**。Taida コードからバックエンドのコード生成段階へ渡る式は、公開関数境界・lambda 境界・型情報の保存先のすべてで具体型に固定されている必要がある。

- `E1525`: 名前付き関数の引数、または `Unknown` を含む式が型注釈なしで公開境界に残った場合。例: `add x y = x + y => :Int` は `x: Int` / `y: Int` のように注釈する。
- `E1526`: `name args = body => :Type` の `=> :Type` が欠けている場合。パーサーが構文末尾で検出し、型チェッカーも構文木由来の未注釈定義を拒否する。
- `E1527`: lambda の引数型が呼び出し文脈から決まらない場合。例: `_ x = x + 1` は `_ x: Int = x + 1` と書く。
- `E1528`: lambda の明示型注釈と呼び出し側が要求する `Function([A], B)` の引数型が一致しない場合。
- `E1529`: 型検査完了後の内部表現に `Unknown` が残った場合。バックエンドは型情報を信頼してコード生成するため、この診断が出たプログラムはコード生成段階に進まない。
- `E1530`: `Name[args]()` の `Name` がモールド / 型 / 関数のいずれとしても解決できない場合。
- `E1531`: `.throw()` に渡した値が `Error` 系ではない場合。

`E1520` は **「値の不在を表す型」の完全排除** 診断。PHILOSOPHY.md I の系「値の不在は値の不在」と II の系「ふくろの中身が変わったら、別のふくろにしまいなおす」を整合的に実装する。

**`@()` という構文を「型」として書くこと自体を禁止する**:

- 戻り型注釈 / 引数型注釈 / 型引数として `@()` / `Unit` / `Void` を書く = 「情報なしを意味する型」の意図表明 → reject
- Taida ではぶちパック値が動的に `@()` になるケースは構造的に発生しない (汎用 filter が Taida には存在せず、ぶちパックのフィールド集合を変える操作は別の具体ぶちパック型を戻り型として定義する → II の系参照)

検出範囲:

1. **R1** 戻り型注釈 `:@()` / `:Unit` / `:Void`
2. **R1 対称版** 引数型注釈 `:@()` / `:Unit` / `:Void`
3. **R2** 関数本体末尾で `@()` / `Unit` / `Void` リテラル
4. **R2 拡張** 注釈なしで関数の最終推論型が `@()` / `Unit` / `Void` に確定 (中間変数経由の抜け道を塞ぐ)
5. **R3** クラスライク型 / モールド / 継承宣言の本体が `@()` で空のケース (`Pilot = @()` / `Int => PilotId = @()` / `Mold[T] => Box[T] = @()` 等) — Parser が即時 reject し、空ぶちパック型をクラスライク経由で導入する経路を閉じる
6. **空ぶちパック型注釈** `x: @()` / `field: @()` — Parser が `[E1520]` を発火し、識別子位置でも空ぶちパック型を書けないようにする
7. **ジェネリック制約** `Mold[T]` で `T` が `@()` / `Unit` / `Void` 等に具象化される
8. **パターンマッチ** で `@()` パターンを書く (warning レベル、要検討)

空リスト `@[]` は「空のリスト」という明確な意味を持つので別物であり、`[E1520]` の対象ではありません。空ぶちパックリテラル `@()` を **値** として書く場面そのものが Taida 言語仕様には存在しません (汎用 filter が存在せず、ぶちパックのフィールド集合を変える操作は別の具体ぶちパック型を戻り型として定義する → II の系参照)。

ぶちパックのフィールド集合を変える操作は、戻り型を別の具体ぶちパック型として定義する (例: `removePrice item: @(name: Str, price: Int) = ... => :@(name: Str)`)。汎用 filter は Taida に存在せず、「フィールドを完全に削り尽くす操作」は戻り型 `:@()` が禁止されるため関数として定義不可能。これにより型と実値のズレが構造的に発生しない設計を保証する。

ClassLike / Inheritance で「親型を継承するだけで自前のフィールドを追加しない」ケースは、`taida-lang/prelude` の `marker: :Type` のような **意味を持つ単一フィールド** を追加するか、共通 `Enum` のバリアントとして表現します。空のまま継承して情報のない型を作ることは認めません。

実装状況: R1 / R1 対称 / R2 / R2 拡張 / R3 / 空ぶちパック型注釈 はすべて実装済みです。関数定義の戻り型・引数型、エラー天井の `error_param` / `return_type`、クラスライク / モールド / 継承定義のフィールド型注釈、`Cage` の `runner` 側の `Out` 型引数、クラスライク / モールド / 継承の空本体で、いずれも `[E1520]` を発火します。`:Async[Unit]` / `:Result[Unit, _]` / `:List[Unit]` / `:Function([Unit], Unit)` / `:@(payload: @())` のようなネスト形も再帰的に検出します。

### 型推論・演算意味論エラー (`E16xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E1601` | 関数 / エラーハンドラの戻り型が宣言と不一致 | TypeChecker |
| `E1602` | BuchiPack / TypeDef の参照先にフィールドが存在しない | TypeChecker |
| `E1603` | 条件分岐の値を返すアーム同士の戻り型が不一致。`throw` だけのアームなど型未確定のアームは基準型にしない | TypeChecker |
| `E1604` | 条件分岐の条件式が `Bool` 型ではない | TypeChecker |
| `E1605` | 比較演算 (`<` / `<=` / `>` / `>=` / `==` / `!=`) のオペランド型が不整合 | TypeChecker |
| `E1606` | 論理演算 (`&&` / `\|\|`) のオペランド型が `Bool` ではない | TypeChecker |
| `E1607` | 単項演算 (`-` / `!`) のオペランド型が不整合 | TypeChecker |
| `E1608` | 未定義の列挙型 / 列挙 variant が参照された | TypeChecker |
| `E1609` | (予約) | — |
| `E1610` | 継承関係 (`Inheritance`) に循環検出 | TypeChecker |
| `E1611` | (予約) | — |
| `E1612` | WASM プロファイルが受け付けない API capability (例: `wasm-min` / `wasm-edge` の `taida-lang/net` `httpServe`) | TypeChecker |
| `E1613` | `TypeExtends` が enum variant リテラルを受け付けない | TypeChecker |
| `E1614` | (tail-only mutual recursion detection guard — 発火は negative 形で検査、ハンドラ経路の保険コード) | TypeChecker |
| `E1615` | (予約) | — |
| `E1616` | 条件分岐のアーム本体で、副作用のみの裸の関数呼び出し文を禁止 | Parser |
| `E1617` | Regex の不変条件違反 (WASM プロファイル下での `Regex` 参照、`__` 接頭辞フィールドの衝突など) | TypeChecker / コード生成 |
| `E1618` | モジュール境界越しの enum variant 並び順不一致 | TypeChecker |
| `E1620` | CPU worker 本体から I/O / 環境 / 時刻 / ネットワーク / プロセス / pool など外部効果を持つ API を呼び出した | TypeChecker |
| `E1621` | CPU worker 本体から addon / host interop 境界 (`RustAddon` / `Cage` / JS runner など) を越えた | TypeChecker |
| `E1622` | CPU worker 本体の中で `AsyncTask` / `Par` / `ParMap` / `Async` / `Stream` などの非同期・並列構造をネストした | TypeChecker |
| `E1623` | CPU worker 本体が `Async` / `AsyncTask` / `Stream` / `Molten` など worker へ転送できない型の値を捕捉した | TypeChecker |
| `E1624` | CPU worker 本体が checker で本体を検証できない関数値を捕捉または呼び出した | TypeChecker |
| `E1625` | (予約) CPU worker 本体による可変グローバル状態の捕捉 | — |
| `E1626` | CPU worker 本体が型未解決の値を捕捉または呼び出した | TypeChecker |
| `E1627` | CPU worker 本体から purity claim のない addon 関数を呼び出した | TypeChecker |
| `E1628` | addon 関数の effective purity claim が active `[parallelism] addon_purity` policy を満たさない | TypeChecker |
| `E1629` | addon audit metadata が無効、期限切れ、失効済み、または検証不能 | TypeChecker |
| `E1630` | addon purity policy または function override が不正 | TypeChecker |
| `E1631` | addon manifest の purity metadata が malformed、または未知関数を参照した | Addon manifest parser / TypeChecker |

#### `E1605` — 比較オペランド型の不整合

`E1605` は比較演算子そのものに対する前段ゲートであり、式ツリーの途中に埋まっていても発火します。対象には `stdout(...)` の引数、ユーザー関数 / メソッド呼び出しの引数、テンプレート補間 (`${...}`)、ぶちパック / 型適用のフィールド値、lambda 本体、条件分岐のアーム本体などが含まれます。

この診断が出たプログラムは Interpreter / Native / WASM の各正式バックエンドに渡されません。Enum やクラスライク型の値を順序比較したい場合は、先に明示的な数値化 API (例: `Ordinal[<enum>]()` など) を使って比較対象の型を揃えてください。

### CLI / モジュール境界エラー (`E17xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E1700` | 提供されていない top-level command / CLI flag が呼ばれた。新しい command path / positional syntax を使うこと | CLI |
| `E1701` | `packages.tdm` で宣言された公開 API とエントリモジュールの実シンボル群が不整合 (未公開 symbol import / 宣言済み symbol 欠如 / module 内シンボル未発見) | TypeChecker |

`E1700` の標準表示:

```text
[E1700] Command '<old>' is not available. Use '<replacement>' instead.
        See `taida --help` for the current command set.
```

提供されていない flag の表示例:

```text
[E1700] Flag '--target <target>' is not available. Use 'taida build <target> <PATH>' instead.
        For example: `taida build native src`.
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
| `E1960` | 内部 `__` フィールドへのユーザ向けドットアクセス禁止。値の取り出しは unmold / `getOrDefault(default)`、失敗詳細は `errorInfo()` を使う | TypeChecker / Runtime |
| `E1961` | Native / WASM handler mode の entry 関数検証失敗。`taida-lang/abi` import、handler の存在、引数数、`WebRequest` 引数型、`WebResponse` 戻り型を検証する | CLI / Build driver |

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
| `E1961` | Native / WASM handler mode の entry 関数検証失敗 |

### 帯域を再利用する予約コード

新帯域を切らず、既存帯域を再利用して採番する予約コードです。

| コード (予約) | 内容 | 既存帯域 |
|--------------|------|---------|
| `E0700` | Native、および Native と同系統のコード生成を行う WASM プロファイルで、相互再帰を検出したときの拒否 | `E07xx` コード生成エラー |
| `E0701` | 直接再帰 (`A → A`) の非末尾位置呼び出しを reject。深い直接再帰はランタイムでスタックオーバーフローするため、コンパイル時に発見する。修正案: 末尾位置に書き換える (`return` 直接) か、accumulator を伴う末尾再帰版を別関数として書く。`If[cond, then, else]()` モールドの引数は末尾位置として扱われない (内部の自己再帰も対象。短絡評価ではあるが runtime の trampoline が `MoldInst` を tail target として認識しないため)。ガード式 `(\| cond \|>)` の各アーム本体や ErrorCeiling handler 末尾は引き続き末尾位置。詳細は `docs/reference/tail_recursion.md` を参照。 | `E07xx` コード生成エラー (verify check / way check) |
| `E1506` | 通常の関数呼び出し引数型不整合。関数値引数では、期待される関数型へ省略推論できない場合も含む | `E15xx` 定義・意味論エラー |
| `E1508` | `Lax[T].getOrDefault` / `map` / `flatMap`、`Result[T, P].getOrDefault` / `map` / `flatMap` / `mapError`、`Async[T].getOrDefault` / `map`、`List[T].fold` / `reduce`、および関数値を受け取るメソッド境界の引数型不整合 (関数引数型ピン違反を含む。`getOrThrow` は arity 0 のため対象外) | `E15xx` 定義・意味論エラー |

### アドオンマニフェストエラー (`E20xx`)

`native/addon.toml` の parser が発射する診断。詳細仕様は
`docs/reference/addon_manifest.md` を参照。

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E2001` | `targets` 配列のエントリが許可リスト（現在は `{"native", "wasm-full"}`）に含まれない | Addon manifest parser |
| `E2002` | `targets = []` — 空配列は許容しない（key を省略するとデフォルト `["native"]` が適用される） | Addon manifest parser |

### host boundary 型制約エラー (`E36xx`)

| コード | メッセージ | フェーズ |
|--------|-----------|---------|
| `E3601` | `Wired[T]` 制約を満たさない型が host wire 境界へ渡された | TypeChecker |
| `E3602` | `HostCall` の `steps` が `HostStep[...]` の list ではない、または `HostStep` list に別種の要素が混在している | TypeChecker |
| `E3603` | `HostCapability[name, kind]()` / `HostStep[method, args]()` の compile-time `Str` identity が解決できない、または active host capability manifest に未宣言である | TypeChecker |

### 自己アップグレード供給網エラー (`E32K1_*`)

`taida upgrade` が GitHub Releases から SHA256SUMS および artifact を取得し、cosign で
verify する経路で発射する診断。`--required` 既定です。

| コード | 発生条件 | 推奨対応 |
|--------|----------|---------|
| `E32K1_UPGRADE_DOWNLOAD_FAILED` | release artifact / SHA256SUMS / cosign bundle の取得 (HTTP / `file://`) が失敗、または HTTP non-2xx | ネットワーク状態を確認し再実行。proxy / firewall がある場合は `https://api.github.com` および `https://github.com/taida-lang/taida/releases/...` への到達性を確認 |
| `E32K1_UPGRADE_STAGE_FAILED` | 取得済み bytes を `/tmp/taida_upgrade_<pid>_<nanos>_*` に書き出す段階で失敗 | `/tmp` の disk 空き容量 / 権限を確認。`TMPDIR` を任意のディレクトリへ変更可 |
| `E32K1_UPGRADE_NO_SHA256SUMS` | release に SHA256SUMS が公開されていない、もしくは対象 archive 行が欠落 | release tag を再確認し、リリースワークフローが SHA256SUMS を上げ直すまで待機 |
| `E32K1_UPGRADE_SHA256_MISMATCH` | 取得 archive の SHA-256 が SHA256SUMS の expected と一致しない | 取得を再試行。`--force-refresh` 等で cache を無効化、それでも mismatch が続く場合は供給網侵害を疑い、手元 binary に切り戻して `taida-lang/taida` security advisory を確認 |
| `E32K1_UPGRADE_SHA256SUMS_INVALID_ENCODING` | cosign 検証通過後の SHA256SUMS が UTF-8 として decode できない | 上記 `_SHA256_MISMATCH` と同様に供給網侵害を疑う。release を再公開する判断は upstream 側 |
| `E32K1_COSIGN_MISSING` | `taida upgrade` 実行環境の `PATH` 上に `cosign` が存在しない | `cosign` (sigstore) を install して `PATH` に通す。現行 Taida では `taida upgrade` の cosign verify は **必須** |
| `E32K1_UPGRADE_SHA256SUMS_COSIGN_MISSING` | `SHA256SUMS.bundle` が release から取得できない | release ワークフローが bundle を上げ直すまで待機 |
| `E32K1_UPGRADE_SHA256SUMS_COSIGN_REJECTED` | cosign が SHA256SUMS の署名を拒否 (identity / certificate-identity-regexp 不一致など) | 公式 release tag であることを確認。手動再取得後も再現する場合は供給網侵害として upstream に報告 |
| `E32K1_UPGRADE_SHA256SUMS_COSIGN_ERROR` | cosign の起動 / 内部エラーで verify が完了しない | `cosign version` を確認し、最新版に更新。`/tmp` の権限不足や AppArmor / SELinux 等の sandbox 制約が原因のことがある |

### ロックファイル整合性エラー (`E32K2_*`)

`taida.lock` (current schema = `sha256:` + 64 hex 必須、addon lock entry は release metadata 必須) の load / migrate / drift 検査が発射する診断。

| コード | 発生条件 | 推奨対応 |
|--------|----------|---------|
| `E32K2_LOCKFILE_V1_REJECTED` | 古い schema (`fnv1a` 等の legacy integrity、または addon metadata を持たない旧形式) の `taida.lock` を load しようとした | `taida ingot migrate-lockfile` で current schema に移行。lockfile を直接書き換えない |
| `E32K2_LOCKFILE_UNSUPPORTED_VERSION` | `taida.lock` schema が現在の taida binary がサポートする `LOCKFILE_SCHEMA_VERSION` より新しい | taida binary を `taida upgrade` で更新するか、互換 lockfile を生成し直す |
| `E32K2_LOCKFILE_INTEGRITY_MISMATCH` | `validate_resolved_bindings` が `--frozen` 非依存で発射する 4 経路: (1) `taida.lock` の `integrity` が `sha256:` 以外の prefix (legacy / unknown algorithm)、(2) resolved package が `taida.lock` から欠落、(3) `(version, source, integrity)` triple が resolver 結果と一致しない、(4) `taida.lock` の package 件数と resolver 件数が不一致 | `taida ingot update` で再解決 (`--frozen` を外す)、または上流の package version pin を見直す。`sha256:` 以外の prefix は `taida ingot migrate-lockfile` 経由で current schema に正規化 |
| `E32K2_LOCKFILE_DRIFT` | `--frozen` 指定で `.taida/taida.lock` が欠落、または `is_up_to_date` 検査で `packages.tdm` と drift | `taida ingot update` で lockfile を再生成して commit。CI で `--frozen` を保つ場合は事前に開発者側で lock を更新 |
| `E32K2_LOCKFILE_MIGRATE_FAIL` | `taida ingot migrate-lockfile` で installed dependency が見つからない、SHA-256 計算に失敗 | `.taida/deps/...` の中身を確認、`taida ingot install` で取得し直す |
| `E32K2_LOCKFILE_ADDON_METADATA_MISSING` | addon lock entry に `published_at` または `publisher_login` がない | lockfile を current schema で再生成する。手書き追記ではなく `taida ingot install` / `taida ingot update` を使う |
| `E32K2_LOCKFILE_ADDON_METADATA_INVALID` | addon release metadata が RFC3339 UTC seconds や GitHub login の形式に合わない、または取得 release metadata が不正 | upstream release metadata を確認し、lockfile を再生成する |
| `E32K2_LOCKFILE_PUBLISHER_MISMATCH` | 既存 lockfile の addon publisher と今回解決した release publisher が一致しない | publisher 変更を監査し、意図した移管なら lockfile を明示更新する |
| `E32K2_LOCKFILE_AGE_REGRESSION` | 既存 lockfile の addon `published_at` より古い公開日時を持つ release metadata が返った | upstream release / registry を確認し、意図しない巻き戻りなら install を中止する |
| `E32K2_FRESH_RELEASE_REFUSED` | third-party addon release が configured release-age window より新しい | publisher と artifact を確認し、必要時のみ `taida ingot install --allow-fresh` で一回だけ override |
| `E32K2_INTEGRITY_UNSUPPORTED_ENTRY` | tarball / extracted dir に non-regular file (symlink / device / fifo 等) が含まれ SHA-256 stream walker が traverse できない | 当該パッケージの公式 archive を確認。手元 fork の場合は構造をフラットなファイル構成へ修正 |

### ソースパッケージ整合性エラー (`E32K3_*`)

ソースパッケージ (`taida-lang/*` の `[packages."x"]`) が SHA-256 pin で
verify されることを保証する診断。`TAIDA_VERIFY_SIGNATURES=required` を
明示した場合は source archive の cosign 検証も hard-fail policy になります。
現行 Taida では `taida-lang/*` 以外のソース pin は受理しません。

| コード | 発生条件 | 推奨対応 |
|--------|----------|---------|
| `E32K3_GITHUB_BASE_URL_CONFINED` | source package install 時に `TAIDA_GITHUB_BASE_URL` が `https://github.com` 以外を指している | 現行 Taida では source archive は `https://github.com` 固定。テスト環境で override する場合は `cargo test` 内の専用 helper を使う |
| `E32K3_NON_OFFICIAL_SOURCE_REJECTED` | `[packages."x"]` table に `taida-lang/*` 以外の owner の source pin を書いた | 現行 Taida では公式 namespace のみ受理する。3rd party は addon (cdylib) として配布する設計 |
| `E32K3_SOURCE_INTEGRITY_MISSING` | `[packages."x"]` table に `integrity` が無い、または `sha256:<64 lowercase hex>` 以外の prefix | `packages.tdm` に `integrity = "sha256:<64 hex>"` を追加。値は対象 tag の source archive SHA-256 を使う |
| `E32K3_SOURCE_INTEGRITY_MISMATCH` | キャッシュ済み source archive / cached store の SHA-256 が `packages.tdm` の expected と一致しない | `taida ingot install --force-refresh` で再取得。再現する場合は供給網侵害を疑う |
| `E32K3_SOURCE_INTEGRITY_UNVERIFIED` | キャッシュ済み source archive に SHA-256 sidecar が無い、または読めない | `taida ingot install --force-refresh` で sidecar を再生成 |
| `E32K3_SOURCE_COSIGN_REQUIRED` | `TAIDA_VERIFY_SIGNATURES=required` 下で source archive の cosign 検証が skip / warn / 失敗 | source archive に対応する cosign bundle が公開されているか確認し、bundle がない配布形態では `TAIDA_VERIFY_SIGNATURES` を未設定または `best-effort` にして SHA-256 pin 検証を trust root にする |
| `E32K3_VERIFY_SIGNATURES_INVALID` | source package install 時の `TAIDA_VERIFY_SIGNATURES` が `required` / `best-effort` / `off` 系の既知値ではない | 値を既知の policy 名に直すか、未設定に戻す |
| `E32K3_PACKAGES_TDM_DUPLICATE_TABLE` | 同一 `[packages."<id>"]` ブロックが `packages.tdm` に複数存在する。後続テーブルで pin が silent に上書きされてレビューで気付けない状態 | 重複ブロックを 1 つにまとめ、source pin が一意になる形に書き直す |

### パッケージ facade 整合性エラー (`E32K4_*`)

`packages.tdm` の公開 facade と entry module の実 export surface が一致することを保証する診断。`taida ingot publish` は tag push の前にこの検査を行います。

| コード | 発生条件 | 推奨対応 |
|--------|----------|---------|
| `E32K4_FACADE_SYMBOL_NOT_PUBLIC` | consumer が import した symbol が `packages.tdm` の facade に含まれていない | import symbol を facade 内の公開名に直すか、publisher 側で意図した公開 symbol を `packages.tdm` に追加する |
| `E32K4_PUBLISH_SYMBOL_NOT_IN_ENTRY` | `packages.tdm` の facade に含まれる symbol を entry module が export していない | entry module の `<<< @(...)` に symbol を追加するか、facade から削除する |
| `E32K4_PUBLISH_SYMBOL_MISSING` | entry module が export する symbol が `packages.tdm` の facade に含まれていない | 公開するなら facade に追加し、非公開なら entry module の `<<< @(...)` から外す |
| `E32K4_PUBLISH_ENTRY_INVALID` | publish 前検査で entry module を読めない、または parse できない | `packages.tdm` の entry 指定と entry module の構文を修正する |

## 帯域ルール

### 帯域の分類と境界

診断コードは発生フェーズに基づいて3つの層に分かれる。

#### 1. 前段ゲート（`taida way check` / `taida way` で検出、バックエンドに到達しない）

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
| `E36xx` | TypeChecker | host boundary 型制約。`Wired[T]` と host step list の境界違反 |

#### 2. Backend 層（コード生成時に検出）

| 帯域 | フェーズ | 責務 |
|------|---------|------|
| `E07xx` | Native / WASM / legacy JS codegen | 生成不能、未対応構文 |

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

**正規形式**: `[E####]`（ブラケット囲み）を正規とする。`E03xx` のコロン形式は後方互換のため解析可能なまま維持するが、新規コードは必ずブラケット形式を使用すること。構造化診断ストリームでは両形式を同じ `code` として正規化して出力するため、CLI / `taida way verify` の JSONL を読み取るツールは形式違いを意識する必要はない。

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
