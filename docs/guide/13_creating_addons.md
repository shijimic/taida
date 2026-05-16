# インゴット内でのアドオン作成

> **PHILOSOPHY.md — IV.** キモい言語だ。だが、書くのも読むのもAI。眺めるのが人間の仕事。

**インゴット (ingot)** はパッケージの単位、**アドオン (addon)** は
インゴットの中に置かれるネイティブローダー層です。アドオンは Rust の
`cdylib` と `native/addon.toml` の組み合わせで構成され、`addon.toml` に
ABI バージョン、パッケージ名、関数表、任意のプレビルド配布メタデータを
記述します。パッケージ系の CLI コマンドは `taida ingot` 配下に集約
されています。

本ガイドはアドオンの *作者* を対象とします。マニフェストの正式な
スキーマは [アドオンマニフェスト](../reference/addon_manifest.md)、
`taida ingot publish` の CLI 仕様は [CLI リファレンス](../reference/cli.md)
を参照してください。

### バックエンドポリシー (どこでアドオンが動くか)

| バックエンド | 状態 | 備考 |
|--------------|------|------|
| インタプリタ | 対応 | アドオンファサードを動的モジュールとして読み込みます。既定ビルドの cdylib は `dlopen` 経由でディスパッチされます。 |
| ネイティブ (AOT) | 対応 | ファサードはビルド時に静的解析され、関数定義は IR 関数に低水準化されます。 |
| JS トランスパイラ | 非対応 | JS 側にディスパッチャがないため、アドオン由来のインポートはエラーになります。 |
| WASM (`wasm-min` / `wasm-wasi` / `wasm-edge`) | 非対応 | これらのプロファイルはアドオンを扱いません。アドオンを扱う WASM プロファイルは `wasm-full` のみです。詳細は [WASM プロファイル](../reference/wasm_profiles.md) を参照してください。 |

インタプリタとネイティブを対象とする作者は、ファサードファイルを 2
通り書く必要はありません。同じ `taida/<stem>.td` が両方で動作します。

### ネイティブバックエンドがファサード内で理解する構文

ファサードローダーは、最上位に次の構文要素を許容します。

- **エイリアス** — `FacadeName <= lowercaseAddonFn`。`lowercaseAddonFn`
  はマニフェストの `[functions]` 表に列挙された関数名です。
- **pack リテラル** — `FacadeName <= @(field <= value, ...)`。
- **スカラ / リスト / 算術 / テンプレートバインディング** —
  `N <= 0`、`msg <= "hello"`、`greet <= \`hi, ${who}\``、算術式、関数
  呼び出し、メソッド呼び出し、フィールドアクセス、モールド / 型の
  インスタンス化。
- **FuncDef** — `Name args = body => :Type`。公開 (`_`-接頭辞なし) /
  非公開 (`_`-接頭辞) のいずれも収集され、非公開のものは、公開
  FuncDef 本体や pack バインディングから到達可能であれば
  summary に昇格します。
- **相対インポート** — `>>> ./child.td => @(Sym1, Sym2)`。非相対
  パスは許容されません。
- **エクスポート宣言** — `<<< @(Sym1, Sym2)`。指定された場合、列挙
  されていないシンボルはユーザーからは名指せません。

現状受理しない構文 (実際のアドオンでは利用されていないため情報提供のみ):

- ファサード内の `TypeDef` / `EnumDef` / `MoldDef` 文。
- `<<< <path>` 形式の再エクスポート。

これらに依存するファサードがある場合、`taida build native` がコンパイル
エラーで指摘します。回避策は、ローダーが理解する純 Taida 表現で
ファサード FuncDef を書き、欠落構文を内側にラップすることです。

公開とリリースのワークフローは **タグ push のみ** です。`taida ingot publish`
は git タグを push して即終了し、アドオンリポジトリ側の CI
(`.github/workflows/release.yml`) がリリースをビルドして公開します。
詳細は [§3 リリースワークフロー](#3-リリースワークフロー) を参照
してください。

---

## 0. はじめの一歩 — `taida init --target rust-addon`

ゼロから公開可能なアドオンへ最短で到達するには、組み込みの雛形を
使います。

```bash
$ taida init --target rust-addon my-addon
```

このコマンドは Rust クレート、Taida ファサード、`addon.toml`、リリース
ワークフローを 1 ステップで配置します。

次にやることは 3 つだけです。

1. `packages.tdm` の `<<<@a owner/name` の `owner` を自分の GitHub
   ユーザー / 組織名に書き換えてコミットする。
2. `cargo build --release` で cdylib がビルドできることを確認する。
3. (任意) `native/addon.toml` の `prebuild.url` を相対 `file://target/release/lib<name>.so`
   に向け、ローカルで `taida ingot install` を試す ([§5](#5-file-を使ったローカルテスト))。

公式同梱アドオンの `taida-lang/terminal` も同じ雛形で構築されており、
リリースワークフローは本リポジトリのテンプレートに 2 つのプレース
ホルダーを当てたものです。

---

## 1. ディレクトリ配置

最小構成のアドオンクレートは、対応するパッケージと並べて置きます。

```
my-addon/
  packages.tdm                  # Taida パッケージマニフェスト
  Cargo.toml                    # cdylib クレート
  src/lib.rs                    # アドオンエントリポイント
  native/
    addon.toml                  # インストール時マニフェスト
  taida/
    my-addon.td                 # 公開ファサード
```

`Cargo.toml` ではクレートを `cdylib` として宣言し、インツリーの
`taida-addon` クレートに依存します。

```toml
[lib]
crate-type = ["cdylib"]
```

---

## 2. `addon.toml` マニフェスト

プレビルド配布を含まない最小構成のマニフェスト (利用者は `.so` を
自分で配置する) は次のとおりです。

```toml
abi = 1
entry = "taida_addon_get_v1"
package = "my-org/my-addon"
library = "my_addon"

[functions]
greet = 1
```

`taida ingot install` でフェッチ可能なプレビルドを配布するには
`[library.prebuild]` セクションを追加します。

```toml
[library.prebuild]
url = "https://github.com/my-org/my-addon/releases/download/{version}/lib{name}-{target}.{ext}"

[library.prebuild.targets]
"x86_64-unknown-linux-gnu"  = "sha256:<小文字 16 進 64 文字>"
"aarch64-unknown-linux-gnu" = "sha256:..."
"x86_64-apple-darwin"       = "sha256:..."
"aarch64-apple-darwin"      = "sha256:..."
"x86_64-pc-windows-msvc"    = "sha256:..."
```

URL テンプレートでは `{version}` / `{target}` / `{ext}` / `{name}` の
4 種類が展開されます。サポートターゲットの全リスト、SHA-256 整合性、
署名セクション (予約) などの詳細は
[アドオンマニフェスト](../reference/addon_manifest.md) を参照
してください。

すべてのターゲット向けにバイナリを配布する必要はありません。検証済み
のものだけで構いません。マニフェストで宣言していないターゲット上の
利用者には、インストール時にサポート対象のターゲットが列挙された
エラーメッセージが返ります。

---

## 3. リリースワークフロー

リリースパイプラインは `.github/workflows/release.yml` の **4 ジョブ
ワークフロー** です。`taida init --target rust-addon` で新規アドオン
クレートを作るとこのワークフローが自動配置されます。

### トリガー

ワークフローは次の 2 つのイベントで発火します。

- Taida のバージョン正規表現
  `^[a-z]\.[0-9]+(\.[a-z0-9][a-z0-9-]*)?$` に一致するタグへの `push`
  (例: `a.1`, `b.3.rc`, `aa.7.beta`)。semver 形式のタグ (`v1.2.3`) は
  意図的に無視されます。
- `tag` 入力付きの `workflow_dispatch` (再実行用)。

### ジョブ構成

| ジョブ | 役割 |
|--------|------|
| `prepare` | タグ正規表現を検証して ref を解決 |
| `gate` | `cargo fmt --check` → `cargo clippy -- -D warnings` → `cargo test` |
| `build` | 5 プラットフォームマトリクスで cdylib をビルド |
| `publish` | アーティファクトを集約し `gh release create` を実行 |

5 プラットフォームのマトリクスは linux-gnu (x86_64 / aarch64)、
darwin (x86_64 / aarch64)、windows-msvc (x86_64) です。

`publish` ジョブは `GH_TOKEN: ${{ github.token }}` で認証されるので、
リリース作成者は常に `github-actions[bot]` です。`taida ingot publish`
を実行した人物がリリース作成者になることはありません。

リリースには 5 個の cdylib に加え、`addon.lock.toml` (CI 生成の
SHA-256 ロック) と `SHA256SUMS` が添付されます。`taida ingot install`
はこれらを正本として参照します。

### 参照実装

`taida-lang/terminal` がこのパイプラインの参照例です。ワークフロー
ファイルもリリースアセット構造も、信頼できる現物として参照できます。

---

## 4. `taida ingot publish` でリリースする

開発者が手で動かすステップは 2 つだけです。

1. `packages.tdm` の `<<<@<version> owner/name` の `<version>` を、
   今回リリースしたい番号に書き換えてコミットする。
2. `taida ingot publish` を実行する。

publish コマンドは次の処理を行います。

- `packages.tdm` の self-identity に書かれた `<version>` を読み取る。
- 直前のリリースタグから HEAD までの API 差分を解析し、本来あるべき
  次バージョンを算出する。
- 両者が一致しなければ、その場で publish を拒否する。
- 一致すれば、その `<version>` を git タグとして origin に push して
  即終了する。

```bash
# 1. プレビュー: どのバージョンが公開されるか
$ taida ingot publish --dry-run
Publish plan for my-org/my-addon:
  Last release tag: a.3
  API diff: added 2
  Next version: a.4
  Tag to push: a.4
  Remote: origin
  Dry-run: no git changes performed.

# 2. 実行: タグを push して即終了
$ taida ingot publish
Created tag 'a.4' and pushed to origin.
CI will build and publish the release.
```

publish は CI の完了を **待ちません**。GitHub Actions のタブで 4
ジョブの進行を確認してください (5 プラットフォームマトリクスで概ね
90 秒程度)。

### `packages.tdm` の `<version>` を何に書き換えるか

`taida ingot publish` が API 差分から算出するルールは次のとおりです。
書き換える `<version>` を予想するときの参考にしてください。

| API 差分 | 次バージョン |
|----------|--------------|
| 初回リリース (前回タグなし) | `a.1` |
| シンボル追加 / 内部変更のみ | 番号繰り上げ (`a.3` → `a.4`) |
| シンボル削除 / 改名 | 世代繰り上げ (`a.3` → `b.1`) |

self-identity と算出値が食い違うと publish は次のメッセージで拒否
します。

```
packages.tdm self-identity '<<<@a.3' does not match the tag to be pushed ('a.4').
Bump the `<<<@a.3` line in packages.tdm to `<<<@a.4` and commit before re-running
`taida ingot publish`.
```

### エスケープハッチ

| フラグ | 用途 |
|--------|------|
| `--force-version a.5` | self-identity と API 差分の双方をスキップし、明示指定したバージョンでタグ付け |
| `--label rc` | プレリリースラベルを付与 (`a.4` → `a.4.rc`) |
| `--retag` | 既に push 済みのタグを強制置換 (API 差分はスキップ) |

`--force-version` と `--retag` は API 差分スナップショットを意図的に
スキップします。旧パッケージ (現行パーサーが拒否する構文を含む可能性
のあるもの) を Taida パーサーを経由せずに再タグ付けできるようにする
ためです。

---

## 5. `file://` を使ったローカルテスト

開発中は GitHub に公開する必要はありません。URL テンプレートを
相対 `file://` パスに向けます。

```toml
[library.prebuild]
url = "file://target/release/libmy_addon.so"

[library.prebuild.targets]
"x86_64-unknown-linux-gnu" = "sha256:<ビルドごとに再計算>"
```

制約:

- `file://` 配下は **相対** パスのみ受け付けます。絶対パスと `..`
  コンポーネントは拒否されます。
- パスは `packages.tdm` を含むプロジェクトルートからの相対で解決
  されます。
- 整合性チェックは引き続き走るため、再ビルドのたびに SHA-256 を
  更新してください。

Taida 本体側でも、サンプルアドオンクレートをこの方法でエンドツーエンド
テストしています。

---

## 6. リリース前チェックリスト

- [ ] `packages.tdm` で qualified 識別子
      (`<<<@<version> <owner>/<name>`) を宣言している。`<owner>/<name>`
      を伴わない bare な `<<<@<version>` は `taida ingot publish` が
      拒否する
- [ ] `packages.tdm` の `<version>` を、今回リリースしたい番号に
      書き換えてコミット済みである
- [ ] `.github/workflows/release.yml` が標準テンプレート (4 ジョブ、
      5 プラットフォームマトリクス) である
- [ ] push 予定のタグが origin にまだ存在しない (あるいは `--retag` を
      意図的に渡している)
- [ ] `cargo build --release` がローカルで通る
- [ ] 開発中にローカル `file://` URL に対して `taida ingot install` が
      エンドツーエンドで完了する
- [ ] `[functions]` 表が `declare_addon!` でエクスポートしたすべての
      シンボルを列挙している
- [ ] README に **最小対応 taida バージョン** を記載している (古い
      taida は未知のマニフェストキーを設計どおり拒否するため。
      詳細は [アドオンマニフェスト](../reference/addon_manifest.md))
