# アドオンマニフェストリファレンス (`native/addon.toml`)

> **PHILOSOPHY.md — IV.** キモい言語だ。だが、書くのも読むのもAI。眺めるのが人間の仕事。

本リファレンスは、Rust アドオン基盤が受け付ける `native/addon.toml`
スキーマを定義します。**インゴット (ingot)** はパッケージ単位、
**アドオン (addon)** はそのパッケージ内に格納されるネイティブローダー
層の名称です。マニフェストファイル名は `addon.toml` のままで、
パッケージ系の CLI コマンドは `taida ingot` 配下に置かれます。
アドオンを書いて配布する手順は
[アドオン作成ガイド](../guide/13_creating_addons.md) を参照してください。

このページは「マニフェストに何を書くか」を **冒頭に最小例** で示し、
そのあとに各キーの仕様、バックエンド対応、セキュリティ規約、エラー
条件を順に並べています。

---

## 1. 最小マニフェスト

`addon.toml` に最低限書く必要があるのは次の 4 つの必須キーと
`[functions]` セクションだけです。利用者が自分で `.so` を配置する
モードはこれだけで動きます。

```toml
abi = 1
entry = "taida_addon_get_v1"
package = "my-org/my-addon"
library = "my_addon"

[functions]
greet = 1
```

意味は次のとおりです。

| キー | 役割 |
|------|------|
| `abi` | ABI バージョン番号 (現在は `1`)。`TAIDA_ADDON_ABI_VERSION` と一致させます |
| `entry` | エントリシンボル名 (現在は `"taida_addon_get_v1"`)。`TAIDA_ADDON_ENTRY_SYMBOL` と一致させます |
| `package` | `"<org>/<name>"` 形式のパッケージ識別子。`packages.tdm` と照合されます |
| `library` | cdylib のファイル名ステム (`lib` 接頭辞と拡張子なし) |
| `[functions]` | 公開する関数名とアリティを `name = arity` の形で列挙 |

これだけ書けば、`cargo build --release` で出力した cdylib を
`.taida/deps/<pkg>/native/lib<name>.<ext>` に手で配置するだけで
動作します。

---

## 2. プレビルドを配布する場合

`taida ingot install` でプレビルド cdylib をフェッチできるようにする
には `[library.prebuild]` を追加します。

```toml
abi = 1
entry = "taida_addon_get_v1"
package = "my-org/my-addon"
library = "my_addon"

[functions]
greet = 1

[library.prebuild]
url = "https://github.com/my-org/my-addon/releases/download/{version}/lib{name}-{target}.{ext}"

[library.prebuild.targets]
"x86_64-unknown-linux-gnu"  = "sha256:<小文字 16 進 64 文字>"
"aarch64-apple-darwin"      = "sha256:<同上>"
```

`url` 内のテンプレート変数 (`{version}` / `{target}` / `{ext}` /
`{name}`) は `taida ingot install` が実行時に展開します。配布対象の
ターゲットだけ `[library.prebuild.targets]` に列挙します。詳細は
[§3.3](#33-libraryprebuild) を参照してください。

---

## 3. 各キーの仕様

### 3.1 必須トップレベルキー

`abi` / `entry` / `package` / `library` はすべて必須です。固定された
ABI v1 定数との不一致はパースエラーになります。

- `abi` は整数。現在の値は `1`。
- `entry` は文字列。現在の値は `"taida_addon_get_v1"`。
- `package` は `"<org>/<name>"` の形式。`packages.tdm` の宣言と一致
  しなければなりません。
- `library` は空でない文字列。`lib` 接頭辞と拡張子を除いたファイル
  ステムです。

### 3.2 `[functions]`

```toml
[functions]
greet = 1
noop  = 0
```

このセクションは最低 1 エントリ必須です。キーは Taida ソースから
呼ばれる関数名、値はアリティ (引数の数) を表す非負整数です。
非整数のアリティと重複キーはパースエラーになります。

### 3.3 `[library.prebuild]`

`taida ingot install` がプレビルド cdylib を取得する場所を宣言する
セクションです。本セクションを省略した場合、アドオンは「開発者が
手動で `.so` を配置する」モードとして扱われます。

```toml
[library.prebuild]
url = "https://example.com/releases/{version}/lib{name}-{target}.{ext}"
allowed_prebuild_hosts = ["example.com"]
```

- `url` は必須で、文字列です。
- テンプレート変数は `{version}` / `{target}` / `{ext}` / `{name}` の
  4 種です。未知の変数や、孤立した `{` / `}`、`{{` / `}}` のエスケープ
  はパース時に拒否されます。
- `allowed_prebuild_hosts` を指定する場合は、スキーマやパスを
  含まない小文字 DNS ホスト名の非空配列にしてください。テンプレート
  展開後、`taida ingot install` はホストがこのリストに含まれない
  HTTPS URL を拒否します。

レジストリ経由のアドオンインストールでは、GitHub Release メタデータ
(リリース時刻と公開者ログイン) が `.taida/taida.lock` に記録され、
次回以降のインストールで一致確認されます。第三者の新規リリースは
所定のクーリングオフ期間が経過するまで既定で拒否されます
(`taida-lang/*` は 0 日、その他は 3 日)。期間の解決順は
`packages.tdm` の `[security] min_release_age` → `~/.taida/config.toml` の
`[security] min_release_age` → 環境変数 `TAIDA_MIN_RELEASE_AGE` →
ビルトイン既定値の順です。`--allow-fresh` を渡すとワンショットで
スキップでき、`.taida/install-audit.log` に記録されます。

#### `[library.prebuild.targets]` (プレビルド指定時は必須)

```toml
[library.prebuild.targets]
"x86_64-unknown-linux-gnu"  = "sha256:abcdef0123...全 64 文字"
"aarch64-apple-darwin"      = "sha256:..."
```

- キーは Taida が受け付ける正準的なターゲットトリプル名にしてください。
- 値は `sha256:` の後に **小文字** 16 進文字をちょうど 64 個続けた
  文字列です。大文字 16 進は拒否されます。
- 未知や非正準のターゲット名はパース時に `PrebuildUnknownTarget` で
  拒否されます。この検査によって、攻撃者が制御するキーが
  `path.join()` に到達する前にキャッシュディレクトリのトラバーサル
  攻撃を止めています。

#### 対応ホストターゲット

| トリプル | 状態 |
|----------|------|
| `x86_64-unknown-linux-gnu` | ベースライン |
| `aarch64-unknown-linux-gnu` | ベースライン |
| `x86_64-apple-darwin` | ベースライン |
| `aarch64-apple-darwin` | ベースライン |
| `x86_64-pc-windows-msvc` | ベースライン |
| `x86_64-unknown-linux-musl` | 拡張 |
| `aarch64-unknown-linux-musl` | 拡張 |
| `i686-unknown-linux-gnu` | 拡張 |
| `riscv64gc-unknown-linux-gnu` | 拡張 |
| `x86_64-unknown-freebsd` | 拡張 |

すべてのターゲット向けにバイナリを配布する必要はありません。検証済み
のものだけで構いません。マニフェストに記載のないターゲットの利用者には
インストール時に `addon is not available for your platform` という
エラーが返り、宣言済みのターゲットが列挙されます。

#### `[library.prebuild.signatures]` (予約)

```toml
[library.prebuild.signatures]
"x86_64-unknown-linux-gnu" = "gpg:<opaque-identifier>"
```

将来の GPG / 分離署名検証のために予約されたセクションです。

- キーは `[library.prebuild.targets]` と同じく正準的なターゲット
  トリプル名。
- 値は `gpg:` の後に空でない印字可能 ASCII (空白や制御文字なし) を
  続けた文字列にしてください。`sigstore:` 等の別プレフィックスは、
  将来のベリファイア用に名前空間を清潔に保つため拒否されます。
- 現状 Taida はこのエントリを **格納するだけ** で、検証は行いません。
  今書いておけば、ベリファイアが land したときに同じフィールドから
  読み取られます。

### 3.4 `targets` (任意、トップレベル)

```toml
targets = ["native"]
```

`targets` は、アドオンが cdylib のディスパッチを期待する Taida
バックエンドの集合を宣言します。ソース上は任意ですが、パース後の
マニフェストでは必ず値が入った状態になります。

- キーを省略した場合は `["native"]` が指定されたものとして扱われ
  ます。
- 許可される値は固定リスト `{"native", "wasm-full"}` のみです。
  それ以外 (例: `"wasm"`, `"wasm-min"`, `"Native"` などの誤入力) は
  パース時に拒否されます。
- 空配列 `targets = []` は拒否されます。既定値を使いたい場合はキー
  ごと省略してください。
- 重複は暗黙に削除されます (`["native", "native"]` は `["native"]` と
  同じ扱い)。
- 値が文字列配列以外 (素の文字列や整数等) だった場合は
  `AddonTargetsTypeMismatch` として拒否されます。

#### 互換性規約

1. **既定値も公開仕様の一部です。** `targets` を省略したマニフェスト
   は現行世代の間ずっと `["native"]` に解決されます。
2. **既定値を変更するときは世代繰り上げを伴います。** 既定値そのものを
   ポイントリリースで黙って変えることはありません。許可リストの拡張は
   世代内で行い得ますが、既存の `targets = ["native"]` を持つマニ
   フェストの動作はそのまま維持されます。
3. **未知のターゲットは早期に拒否します。** パーサーは未知エントリを
   `[E2001]` で拒否し、既定値にフォールバックすることはありません。

#### 診断コード

| コード | バリアント | 発射条件 |
|--------|-----------|----------|
| `E2001` | `UnknownAddonTarget` | `targets` のエントリが許可リスト (`{"native", "wasm-full"}`) に含まれない |
| `E2002` | `EmptyAddonTargets` | `targets = []` (空配列) |
| (なし) | `AddonTargetsTypeMismatch` | `targets` の値が配列以外 |

---

## 4. バックエンド対応ポリシー

マニフェストスキーマはバックエンドごとに変わりません。同じ
`native/addon.toml` がアドオンを扱うすべてのバックエンドにとっての
正本です。バックエンドごとに違うのは、アドオン由来のインポートを
ディスパッチャ経由で処理するかどうかです。

| バックエンド | 対応 | 補足 |
|--------------|------|------|
| インタプリタ | 対応 | `feature = "native"` を有効化したインタプリタビルド (既定ビルド) で `dlopen` 経由にディスパッチします。アドオンファサードは専用環境上の動的 Taida モジュールとして動作します |
| ネイティブ (AOT) | 対応 | ビルド時にネイティブコード向けに変換されます。ファサードは静的解析で要約され、関数定義は内部の中間表現に置き換えられます。ぶちパックや定数バインディングはモジュール初期化パスへ移され、cdylib 呼び出しはホスト側ディスパッチャを経由します |
| WasmFull | 対応 | ネイティブ / インタプリタと同じレジストリとファサード経路を再利用します。マニフェスト作者は `targets` 配列に `"wasm-full"` を含めることで明示的に有効化します。cdylib のロード自体はホスト側のネイティブローダーを経由します |
| JS トランスパイラ | 非対応 | JS 側にディスパッチャが存在しません。インポート時に明示的なエラーが発射されます |
| WasmMin / WasmWasi / WasmEdge | 非対応 | これらのプロファイルはアドオンディスパッチャを持ちません。`wasm-full` の別名ではないので、`targets = ["wasm-full"]` が宣言されていても拒否されます |

### 非対応バックエンドのエラー文面

非対応バックエンドで出るエラーメッセージは固定です。

```
addon-backed package 'X' is not supported on backend 'Y' (supported:
interpreter, native, wasm-full). Run 'taida build native' or
use the interpreter; for wasm targets, only 'wasm-full' supports addons.
```

このメッセージに合わせて条件分岐するツールは、
`"supported: interpreter, native"` プレフィックスへの一致を優先して
ください。同プレフィックスは安定仕様の一部で、リテラル一致を前提と
した実装が許されます。括弧内の末尾リストは追加方向に変化し得ますが、
既存のプレフィックス一致はそのまま動作します。

ネイティブバックエンドの静的解析器がファサード内で許容する構文要素は、
作成者視点で [アドオン作成ガイド](../guide/13_creating_addons.md) に
まとめています。

---

## 5. ABI 互換性と前方互換ポリシー

### ABI 互換性

アドオン ABI のバージョン (`TaidaHostV1`、エクスポートシンボル、
呼び出し規約) は世代内で固定です。互換のある追加 (vtable 末尾への
新しいコールバックの追加、新しい省略可能なエクスポートシンボル) は
ビルド番号の繰り上げで land できます。既存スロットの並び替えや
改名、シグネチャの変更は公開仕様を壊す変更として扱い、世代繰り上げ
が必要です。

ABI のメジャー版自体は安定仕様の保証対象には含めません。メジャー
改訂が必要な場合は世代をまたぎます。互換性判断の枠組み全体は
[リリースプロセス](release_process.md) を参照してください。

### 未知キーへの前方互換

マニフェストパーサーは **ストリクト** です。本書に記載のない
セクションヘッダおよびトップレベルキーはすべてパースエラーになります。
これは意図的な ABI ドリフトガードです。

- 未知セクション (例: `[library.experimental]`) は拒否されます。
- 未知のトップレベルキー (例: `maintainer = "..."`) は拒否されます。
- 既知セクション内の未知キーも拒否されます。
- 重複キー (`[functions]` 内を含む) は拒否されます。

マニフェスト作者が知っておくべき前方互換ルールは次の 3 点です。

1. **本リファレンスへのセクション追加は ABI 繰り上げと等価です。** その
   セクションを理解する taida リリースを待ってから使い、対応する
   最小 taida バージョンをアドオン README に記載してください。
2. **既存の予約済みセクション内に任意キーを追加する場合も ABI 繰り上げ
   です。** 古い taida は新キーを含むマニフェストの読み込みを拒否
   します。
3. **未来のキーを書いたマニフェストが古い taida で失敗するのは仕様
   です。** 黙って許容してしまうと、後のバージョンで動作の根幹に
   関わるようになったキーが、古い taida を使い続ける利用者にとって
   暗黙の破壊変更になってしまうためです。

---

## 6. インストール時のセキュリティ規約

### 6.1 HTTPS ダウンロードの上限

`taida ingot install` がプレビルドを HTTPS でダウンロードするとき、
HTTP クライアントは次のポリシーを明示的に適用します。

| ポリシー | 値 | 補足 |
|----------|-----|------|
| リクエストタイムアウト | 120 秒 | エンドツーエンド (リクエスト全体に適用) |
| 最大リダイレクト数 | 10 | 10 ホップを超えた時点で打ち切ります |
| 最大ペイロード | 100 MB | `Content-Length` が上限超過なら開始前に拒否。ストリーミング本体は 100 MB で打ち切り |
| HTTP へのダウングレード | 拒否 | `https → http` 遷移はブロックされます |
| スキーム許可リスト | `https://` / `file://` | それ以外 (`http://` を含む) はネットワーク呼び出し前に拒否 |

10 ホップを超えるリダイレクトチェーンは、無限ループではなく明示的な
`DownloadFailed` エラーになります。上限は一般的な CDN リダイレクト
(GitHub → CDN → オブジェクトストア) を許容しつつ、リダイレクトループ
を早期に検出できるよう設定されています。

### 6.2 `file://` の制約

`file://` URL に対するフェッチャは次を拒否します。

- 絶対パス (例: `file:///etc/passwd`)
- `..` コンポーネントを含むパス (パストラバーサル防止)
- `file://` と `https://` 以外のスキーム

これらのチェックは、いかなるファイルシステムアクセスやネットワーク
I/O **よりも前** に実行されます。

### 6.3 インストールスクリプトは持たない

アドオンマニフェストは `postinstall` / `install` / `scripts` 等の
インストール時コマンドフックを一切サポートしません。ストリクト
パーサーが未知のトップレベルキーと未知のセクションをまるごと拒否する
ため、未来形のキーを書き足してシェルコマンドを忍ばせることもできません。

プレビルドインストール時に行われる処理は以下に限定されます。

1. マニフェストとターゲットトリプルを解決する。
2. 宣言されたプレビルド成果物を取得 / コピーする。
3. 記録された digest とリリースポリシーを検証する。
4. cdylib を依存ツリーに配置する。

ローカルソースビルドは `--allow-local-addon-build` をユーザーが明示的に
指定したときにのみ実施され、integrity の不一致がローカルビルドに
フォールバックすることもありません。

---

## 7. ソースパッケージ整合性と改行コード

`taida ingot install --frozen` は、ローカルソースパッケージツリーを
次のコンテンツハッシュで検証します。相対パスは事前にソート済みです。

```text
sha256(<relative-path> || 0x00 || <bytes>)
```

ハッシュは改行コードを正規化 **しません**。CRLF でチェックアウトされた
ファイルと、同じファイルを LF でチェックアウトしたものは、別の
パッケージ内容として扱われ、ロックファイル上の integrity 値も異なります。
クロスプラットフォームで frozen install を機能させたいパッケージ
作者は、`.gitattributes` でテキスト正規化を固定してください。

```gitattributes
*.td text eol=lf
*.tdm text eol=lf
native/addon.toml text eol=lf
```

---

## 8. ストアサイドカー `_meta.toml`

`taida ingot install` は、抽出された store パッケージの隣に
`~/.taida/store/<org>/<name>/<version>/_meta.toml` という由来サイドカーを
書き出します。サイドカーは自動生成で、手で編集する想定ではありません。

```toml
# auto-generated by taida ingot install
# Do not edit by hand.
schema_version = 1
commit_sha = "<バージョンタグが指す 40 文字 16 進コミット SHA>"
tarball_sha256 = "<抽出前 tarball の 64 文字 16 進 SHA-256>"
fetched_at = "<RFC-3339 UTC タイムスタンプ>"
source = "github:<org>/<name>"
version = "<要求されたバージョン文字列>"
```

| フィールド | 用途 |
|------------|------|
| `schema_version` | フォーマットバージョン (現状 `1`)。将来のスキーマ繰り上げは `UnknownMetaSchema` で検出され、強制的に悲観的リフレッシュを行います |
| `commit_sha` | 最後にフェッチした時点でバージョンタグが指していたコミット SHA。空文字列はフェッチ時に SHA が不明だったことを示し (例: 初回インストール)、次回インストール時に悲観的リフレッシュで補完されます |
| `tarball_sha256` | 抽出前 tarball の SHA-256 |
| `tarball_etag` | 任意の HTTP ETag。値がない場合はフィールドごと省略されます |
| `fetched_at` | フェッチ時刻の RFC-3339 UTC タイムスタンプ (秒単位) |
| `source` | 由来識別子 (例: `github:<org>/<name>`) |
| `version` | 要求されたバージョン文字列 |

サイドカーはその後の `taida ingot install` 呼び出しごとに参照され、
キャッシュエントリの有効性判断に使われます。判定表は
[CLI リファレンス](cli.md#ingot-install) を参照してください。
アドオンマニフェストスキーマ自体 (`native/addon.toml`) はサイドカーの
影響を受けません。サイドカーは公開パッケージの中ではなく、ストア
キャッシュ側に置かれます。

---

## 9. エラー条件

`native/addon.toml` のパースまたはバリデーションで発生するエラーは、
常に `addon manifest error:` で始まるメッセージで報告されます。
`taida ingot install` / `taida ingot publish` などの CLI コマンドは
このプレフィックスを保ったまま、後続に具体的な原因文を連結して表示
します。検出される代表的な失敗条件は次のとおりです。

| 区分 | 失敗条件 |
|------|----------|
| ファイル | マニフェストファイルが読み込めない |
| 構文 | 受理されるストリクト TOML サブセットの外側にある記述 |
| 必須キー | `abi` / `entry` / `package` / `library` のいずれかが欠落 |
| ABI | `abi` の値が現行 Taida リリースのアドオン ABI と一致しない |
| エントリ | `entry` の値が現行 Taida リリースのエクスポートシンボル名と一致しない |
| 識別子 | `package` / `library` が空文字列 |
| 関数表 | `[functions]` セクションが欠落、もしくは空 |
| アリティ | 関数アリティが非負整数でない |
| 型 | キーの値が宣言された型と異なる |
| プレビルド | `[library.prebuild]` が `url` 無しで存在する |
| SHA-256 | `targets.*` が `sha256:` + 小文字 16 進 64 文字の形式でない |
| URL 変数 | `{foo}` が `{version|target|ext|name}` の範囲外 |
| URL 構文 | URL テンプレートに孤立した `{` または `}` がある |
| 未知キー | `[library.prebuild]` 配下に未知キー |
| 許可ホスト | `allowed_prebuild_hosts` に不正なホスト |
| ターゲット重複 | `[library.prebuild.targets]` に同じターゲットが重複 |
| ターゲット未知 | ターゲットキーが Taida の正準トリプル集合に含まれない |
| 署名形式 | 署名値が `gpg:<opaque>` 形式でない |
| 署名キー | 署名セクションのキーが正準トリプルでない、もしくは重複 |
| アドオンターゲット (`E2001`) | トップレベル `targets` のエントリが許可リスト外 |
| アドオンターゲット (`E2002`) | トップレベル `targets = []` (空配列) |
| アドオンターゲット型 | トップレベル `targets` の値が文字列配列でない |
