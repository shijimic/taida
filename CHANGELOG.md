# Changelog

本ファイルは Taida Lang の公開リリースノートです。各リリースの
正本は GitHub Releases であり、本ファイルはリリースに含まれた
ユーザー視点の変更点を要約します。

リリース識別子は Taida のバージョン記法
`@<世代>.<番号>[.<ラベル>]` に従います。詳細は
[`docs/reference/release_process.md`](docs/reference/release_process.md)
を参照してください。`Cargo.toml` の semver は Rust ツーリングのため
のメタデータであり、Taida バージョンの正本ではありません。

過去タグの land 履歴・添付アセット・タグ作成日時は GitHub の
Releases ページが正本です。本ファイルでは現在進行中の次リリースに
向けた変更点と、直近リリースの要約を、利用者視点で記述します。

---

## Unreleased

次リリースに向けた変更点はまだありません。

---

## @f.62 — 2026-06-12

実プロダクション 2 件（自律開発システムと WASM ウェブサービス）を Taida で
構築して発見されたブロッカーの一掃と、その経験から設計確定した言語進化を
まとめたリリースです。

### Added

- **CageBuilder チェーン**: `Cage[db]() => InCage[_, "prepare", @[sql]]()
  => Uncage[_, "all", Out]() >=> rows`。チェーンは記述の構築であり、host
  call は `Uncage` で 1 回だけ発行されます。wire 形（steps 配列の 1
  envelope）は直接形 `Cage[subject, HostCall[...]]()` と同一です。builder
  は値セマンティクスを持つ一級値で、束縛したベースチェーンを軽量な
  prepared statement として再利用できます。
- **Schema-passing generics**: `Uncage` / `HostCall` の Out スロットに
  ジェネリック関数の型変数を書けます。Out スキーマは呼び出しごとの
  compile-time 値（hidden schema parameter の辞書渡し）で、呼び出しは
  明示的型引数が必須です。型変数の転送はホップ数やモジュール境界
  （`>>>` で import した汎用関数への転送）によらず推移的に追跡されます。
- **明示的型引数呼び出し**: `genfn[T1, T2](args)` が宣言された型パラメータ
  を直接束縛します。戻り位置にしか現れない型パラメータも使えます。
- 順序比較の **`Lte` / `Between` モールド**（Str の順序比較を含む。
  Enum を跨ぐ operand は reject）。
- **型シグネチャ付き複文 lambda**。
- **typed unmold bindings**（`name: T <=< expr` / `expr >=> name: T`）、
  **リスト型エイリアス**（`Pairs = @[@(name: Str, value: Str)]`）、
  期待型から空リテラルの型を確定する**双方向推論**。
- **`exit(code)`** が全バックエンドに実装されました（checker だけが型を
  知っている状態でした — インタプリタは未定義変数、native はクラッシュ）。
- インタプリタの**再帰深度上限が 8192** になり（専用の大スタック評価
  スレッド）、native の深度超過はクラッシュではなく診断付き終了になりました。
- 新診断: `[E1543]` / `[E1544]`（パイプライン・プレースホルダの 2 規則）、
  `[E1545]`（非モールド値の unmold）、`[E1546]`（値位置の bare mold）。

### Changed

- **非 monadic 値への `>=>` / `<=<` / `.unmold()` は素通しせずゴリラ**に
  なりました（`[E1545]`、4 バックエンド同形の診断 + `><` + exit 1）。
  全値モールドの正規イディオム `Mold[...]() >=> x` は従来どおり動作します。
- **パイプライン・プレースホルダは例外なしの 2 規則**になりました
  （`_` 1 個 = 構文注入 / なし = 評価して関数なら適用）。クランプ等の
  慣用句は束縛転送形（`x => v => If[v > 100, 100, v]()`）へ。
- **ゴリラ `><` の終了コードは全バックエンドで 1** です（インタプリタは
  0 で終了していました）。
- ユーザー定義モールドの `unmold` フックが native / WASM でも実行されます。

### Fixed

- import したモジュール内の wrapper 経由相互末尾再帰が 2 ホップ目で
  `Undefined variable` になる問題（Critical）と、native の重なり相互
  サイクルの過剰な前方参照エラー。
- 単行 cond アーム本体のパイプ継続が cond の parse を壊す問題。
- プロセス argv への taida 自身のフラグ混入、クロスモジュール戻り型の
  未解決、REPL の深い再帰でのクラッシュ、`Stat` 結果フィールドの型付け、
  JSON import スキーマの解決。
- CageBuilder への不正入力（Unknown 型による checker 迂回）が 3 バック
  エンドで同一の報告（`Runtime error: ..., got <Type>`、exit 1）に
  なりました — native は生ポインタ走査（クラッシュの可能性）、WASM は
  無言の builder 汚染でした。
- 非ジェネリック関数への bracket と `()` 引数の併用（`fn[1](2)`）は
  4 バックエンドすべてでビルド時 / 実行時に reject されます（bracket 値
  の黙殺を廃止）。
- wasm-edge: throw 後も実行が継続する error-flag モデルで、dead path 上の
  後続 `Cage` が pending envelope を上書きし、replay payload を順序ズレで
  消費する問題。host が誤った call を dispatch し、非冪等な host call が
  再実行されていました（症状: 複数 host call を跨ぐハンドラの 500 と D1
  文の二重実行）。dead path 上の cage は不活性になり、native の「最初の
  throw が勝つ」セマンティクスに一致します。

## @f.61 — 2026-06-11

`@f.49` 以降に land した変更をまとめた安定リリースです。バックエンド
パリティの強化 — 参照実装であるインタプリタへの意味論整列 — と、
それに伴う性能改善・診断の拡充が中心です。

### Added

- `taida-lang/js` に `JSCallAsync` を追加。Promise を返す JavaScript
  関数は `Cage[subject, JSCallAsync[path, args, Out]()]()` で呼び、
  `Async[Out]` として待てます。Promise rejection は既存の
  `Async` / `|==` エラー処理に接続されます。
- `Async[T]` を「未来に値が得られる処理」一般として扱う説明に更新し、
  CPU 計算を `AsyncTask` / `Par` / `ParMap` で明示的に遅延・合成する
  public surface を追加。`Par` / `ParMap` は入力順の結果リストを返し、
  worker 境界を越えられない本体や捕捉は checker 診断で reject します。
- native / WASM edge ビルド向けのリクエストハンドラ ABI を追加。
  ハンドラエントリの検証、リクエスト / レスポンスの実体化、サイズ
  上限付きリクエストボディ、世代チェック付きレスポンスハンドルを
  含みます。あわせてホスト capability surface（`taida-lang/abi` の
  capability checker、WASM edge ビルドへのホスト capability 注入、
  ハンドラセッションのポーリング export）を追加。
- `taida-lang/build` が正式に import 可能になりました（インタプリタ /
  native / ディスクリプタドライバ）。従来どおり import なしの利用も
  そのまま動作します。`taida-lang/*` コア同梱パッケージの export は
  単一カタログに統一され、未知シンボル import の診断が全パッケージで
  一様になりました。
- `taida-lang/crypto` を実用的なプリミティブ集合に拡充: `sha512` /
  `sha384` / `sha224`、`hmacSha256`、`constantTimeEquals`、hex /
  base64 エンコード・デコード、`randomBytes` を 4 バックエンドで
  提供します（WASM はプロファイルに応じた gating 付き）。
- 秘密情報を不透明に保持する sealed carrier 型 `Moltenized[T]` と、
  そのセキュリティポリシー子型 `Secret[T]` を追加。表示
  (`[E1533]`)・JSON 直列化 (`[E1534]`)・直接 unmold (`[E1535]`)・
  等価比較 / コレクション帰属 (`[E1536]`) をコンパイル時に reject
  し、実行時も全バックエンドで fail-closed に遮断します。
  `HmacSha256` / `ConstantTimeEq` は sealed buffer を平文に戻さず
  参照で消費します。
- 型 checker の診断を拡充:
  - `[E1532]` ビルドディスクリプタの実行時値としての利用を reject
  - `[E1537]` `Num`（ジェネリック制約マーカー）の値型注釈への誤用を
    reject
  - `[E1538]` プリミティブ型名でのクラスライク / Mold / Enum 定義
    （組み込みが常に勝ち、参照不能になる）を reject
  - `[E1539]` トップレベル実行文の前方関数参照（実行時に未定義に
    なる）を reject
  - `[E1540]` 値の不在を表す予約語（`null` / `undefined` / `none` /
    `nil` / `unit` / `void`）を識別子の定義位置に使うことを reject
  - `[E1541]` `JSON[raw, Schema]()` の未定義スキーマ名を reject
  - `[E1542]` 未定義変数の参照を専用コードに分離

### Changed

- 公開ドキュメントの語彙を現行 Taida invariant に揃え、廃止済みの値
  省略コンストラクタ表記を
  [`docs/api/prelude.md`](docs/api/prelude.md) から取り除き、値省略 /
  失敗の表現を `Lax[value]()` 一手に集約することを明示。到達不能な
  制御を指す他言語由来の表現と、checker の見落としを指す他言語由来
  の表現を、それぞれ「制御が到達しない」「reject 漏れ」のような
  Taida 流の語彙へ書き換え
  ([`docs/api/prelude.md`](docs/api/prelude.md) /
  [`docs/guide/07_control_flow.md`](docs/guide/07_control_flow.md))。
- 番号 04 を共有する
  [`docs/guide/04_buchi_pack.md`](docs/guide/04_buchi_pack.md) と
  [`docs/guide/04_class_like.md`](docs/guide/04_class_like.md) の
  冒頭に推奨読書順序のリードを追加し、値リテラル (`@(...)`) と
  型定義 (`Pilot = @(...)`) の章を読み手がどう辿るかを明示。
- 公開ドキュメント全体の日本語品質を統一。実装側の英語用語を利用者
  視点の日本語表現に置き換え、長い複文を 1 文 1 メッセージに分割し、
  説明用の英語フレーズを段階的な日本語の手順説明へ展開
  ([`docs/guide/`](docs/guide/) /
  [`docs/reference/`](docs/reference/) /
  [`docs/api/`](docs/api/) の各ファイル)。
- 公式ソースアーカイブの署名検証は `TAIDA_VERIFY_SIGNATURES=required`
  または `TAIDA_VERIFY_SIGNATURES=best-effort` で明示した場合に実行する
  opt-in policy に整理。通常の source install は引き続き `github.com`
  の公式パスに固定され、検証 policy は環境変数で明示します。
- `taida-lang/pool` を待機セマフォとして再構成。枯渇時の acquire は
  ブロックして待機し、`waiting` ヘルスカウントで観測できます。解放
  済みリソースは `Lax[Resource]` として返り、acquire のタイムアウト
  省略時はプール設定の `acquireTimeoutMs` に従います。
- HTTP/2 / HTTP/3 のリクエストボディが HTTP/1.1 と同一の streaming
  reader API で観測可能になりました（16MiB 上限と未読時の返却
  セマンティクスは不変）。
- 異種コンテナの要素 kind 追跡を native / WASM ランタイムに追加。
  `Bool` と `Int` の区別、数値境界（`Int` / `Float`）の統一、型の
  異なる同序数 Enum の区別が、Set 演算・`unique`・構造的等価・`|==`
  で 4 バックエンド一致になりました。
- 文字列の index 系 API（`length_` / `get` / `CharAt` / `Slice` /
  `Reverse` 等）を Unicode コードポイント単位に統一。マルチバイト
  文字を含む文字列でも 4 バックエンドが同じ位置・同じ長さを報告
  します。
- `Slice` の境界セマンティクスを全バックエンド・全対象型（`Str` /
  `List` / `Bytes`）で統一。終端の省略は末尾までを意味し、明示的な
  負の終端は空スライスに clamp されます。
- 捕捉されない `throw` の報告を 4 バックエンドで統一。
  `Runtime error: Unhandled error: ...` 形式で stderr に出力し、
  終了コード 1 で停止します。
- `HashMap` / `Set` の表示形を公開コンストラクタ形
  （`HashMap({...})` / `Set({...})`）に統一。
- `Int` / `Float` への数値 parse の受理規律を 4 バックエンドで統一
  （空白・符号・桁あふれ・非有限値の扱い）。

### Fixed

- native / WASM で、プロセスのロードベースを跨ぐ大整数が文字列と
  誤分類され、表示・ハッシュ・JSON・等価比較の多態経路が壊れる
  問題を修正。文字列識別はヒューリスティックではなく magic header
  による正の判定になりました。
- native / WASM のテンプレート文字列補間と、パイプライン束縛
  （`=> name`）/ placeholder の lowering を修正。束縛名が後続の
  同名参照を上書きする問題は capture-avoiding rename で解消しました。
- 2^53 を超える数値域で、`Set` / `unique` の要素同一性が等価比較と
  乖離する問題を修正（要素の fingerprint を等価比較と同じ f64
  domain に正準化）。
- `README.md` を現行の公開 API に追従させ、docs 導線を最新化。
- [`docs/reference/memory_model.md`](docs/reference/memory_model.md)
  を新規追加。正式バックエンドのメモリ管理戦略と、アドオン作者向けの
  所有権規約を明文化。
- [`docs/api/`](docs/api/) を新設し、パッケージ API リファレンスを
  言語リファレンス (`docs/reference/`) から分離。`taida-lang/os` /
  `taida-lang/net` の API 仕様と、コア同梱パッケージの API リファレンス
  索引 ([`docs/api/README.md`](docs/api/README.md)) を集約。
- [`docs/reference/addon_manifest.md`](docs/reference/addon_manifest.md)
  と [`docs/guide/13_creating_addons.md`](docs/guide/13_creating_addons.md)
  を自然な日本語に書き直し。
- 公開リファレンス各所に残っていた内部実装ファイルパス / 内部 Rust
  シンボル / 開発者向け執筆規約を整理し、利用者視点で読めるよう純化。
- [`docs/reference/README.md`](docs/reference/README.md) を利用者向け
  リファレンス索引として整理。

### Performance

- native コード生成の最適化レベルを引き上げ（Cranelift
  `opt_level=speed`。従来は既定の `none`）。
- native ランタイムのホットパスを構造的に最適化: unmold ディス
  パッチの単一 probe 化、arena containment の O(1) reject、末尾
  再帰ループの iteration 単位 arena 巻き戻し、直接形 unmold の融合
  （`Lax[x]() >=> v` のパススルー化、Int リテラル除数の `Div` /
  `Mod` の正確な除算への直下げ）。
- 末尾再帰の自己 append パターンを 4 バックエンドで in-place consume
  に lowering し、リスト蓄積の O(n²) を排除。`Float` を含む Set
  演算・`unique` も fingerprint 経路で O(n) になりました。
- JS / WASM バックエンドの文字列・リスト操作を大幅に高速化。

### Security

- 公式 `taida-lang/*` source archive の cosign 署名検証デフォルトを
  required から disabled (opt-in) に緩和。過去のリリースと同等の保護を
  維持するには `TAIDA_VERIFY_SIGNATURES=required` を設定してください。
- リリース CI の secret スキャンが、スキャナ未導入の環境で検査を
  スキップしたまま成功扱いになっていた問題を修正。スキャナの存在を
  必須化し、CI への導入手順を固定しました。
- `taida upgrade` が、実行中のバイナリより古い公開リリースを「新
  バージョン」として提案する経路を遮断（リリース前ビルドや rollback
  直後に、正規署名付きのダウングレードが静かに成立することを防止）。
  `--version` の明示指定による意図的な rollback は引き続き可能です。

---

## 過去のリリース

公開済みリリースのタグ別変更点・添付アセット・リリース日時は、
本リポジトリの GitHub Releases ページを正本とします。
