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
向けた変更点を、利用者視点で記述します。

---

## Unreleased

次リリースに向けた変更点を、利用者視点で記述してください。

### Added

- (該当なし)

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

### Fixed

- (該当なし)

- `README.md` を現行の公開 API に追従させ、docs 導線を最新化。
- [`docs/reference/memory_model.md`](docs/reference/memory_model.md)
  を新規追加。4 バックエンド (インタプリタ / ネイティブ / JS / WASM)
  のメモリ管理戦略と、アドオン作者向けの所有権規約を明文化。
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

### Security

- (該当なし)

---

## 過去のリリース

公開済みリリースのタグ別変更点・添付アセット・リリース日時は、
本リポジトリの GitHub Releases ページを正本とします。
