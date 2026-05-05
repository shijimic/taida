# `docs/reference/` 執筆ガイド

> **PHILOSOPHY.md — IV.** キモい言語だ。だが、書くのも読むのもAI。眺めるのが人間の仕事。

このディレクトリは Taida Lang の **言語リファレンス**を集めた場所です。
`docs/guide/` (ナラティブの学習用ガイド) や `CHANGELOG.md` (タグ別の land
履歴) とは責務を分けて運用しています。

---

## 1. リファレンスの責務

`docs/reference/*.md` は **現在の安定 surface の定義**のみを記述します:

- 演算子・mold・型・API シグネチャ
- 診断コード (`E####`) の体系と帯域ルール
- CLI コマンドの入出力仕様
- マニフェストファイル / メタデータの schema
- 各 backend の挙動差 (現時点での結論のみ、land 経緯は書かない)

リファレンスを開いた読者が知りたいのは **「今この言語はどう動くか」**
です。歴史的経緯を辿るための場所ではありません。

---

## 2. リファレンスに書いてはいけないもの

以下の要素は **絶対に書かない** でください。レビューで検出され次第、
削除または `CHANGELOG.md` への migrate 対象になります。

| 禁止要素 | 理由 | 行き先 |
|---------|------|--------|
| Blocker ID (`<TRACK>B-<NUM>` 形式の内部追跡ラベル全般) | 進行中・終了済の追跡情報であり、表面仕様ではない | `CHANGELOG.md` (公開する場合) または内部追跡 |
| RC タグ別の着地ナラティブ (`@<gen>.<num>.rc<n>` 単位の時系列記述) | 次の RC で陳腐化する | `CHANGELOG.md` |
| `[FIXED]` / `OPEN` / `PARTIAL` / `tentative` などの状態マーカー | 進行状況。RC ごとに変わる | 内部追跡 |
| 廃止タグの履歴 (どの RC で廃止 / 削除予定かといった記述) | 廃止 *時期* は CHANGELOG 領域 | `CHANGELOG.md` |
| 固定の日付 / タイムスタンプ (`更新日: ...` 等) | 静的に古くなる | プレースホルダ (`<ISO-8601 timestamp>`) |
| 固定の Semantic Versioning 値 (`taida_version: "0.1.0"` 等) | Taida 採番ルール (`@<gen>.<num>.<label?>`) と矛盾 | プレースホルダ |
| `Round N` / `wA`〜`wZ` などの開発時スプリント名 | 表面に登場すべきでない | `CHANGELOG.md` |

廃止された surface を文書化したい場合は、リファレンス末尾に
`## Deprecated` 節を設け、**「現在非サポート」** とだけ書きます。
廃止された具体的な RC タグや経緯は CHANGELOG cross-reference にとどめ、
リファレンス本文に持ち込まないでください。

---

## 3. justified exception

下記は禁止要素のパターンに見えますが、**現在の動作を説明するために
本質的に必要**なため例外として許容します:

- **採番ルール例** (`docs/reference/naming_conventions.md` / `docs/reference/operators.md`):
  `@a.1`, `@b.1.breaking`, `@x.34.gen-2-stable` 等は version 文字列の
  構文を説明するためのプレースホルダ的サンプル。実在のタグを参照して
  いるわけではない。`naming_conventions.md` 末尾の Versioning 節 / `operators.md` の `>>>` / `<<<` モジュール演算子節が該当。
- **CLI フラグの構文例** (`docs/reference/cli.md` `--version <VERSION>` / `<<<@a.1` 形式):
  `--version @b.10.rc2`, `<<<@a.1 owner/name`, `@gen.num` 等は flag / 識別子構文を
  示すサンプル。実在 release への参照ではない。`taida ingot publish` / `taida ingot cache`
  / `taida ingot install` の章が該当。
- **package version 構文サンプル** (`docs/reference/standard_library.md`
  の crypto 例 / `docs/guide/10_modules.md` の packages.tdm 章 /
  `docs/guide/13_creating_addons.md` の `packages.tdm` migration ステップ):
  `>>> taida-lang/crypto@a.1 => @(sha256)`, `>>> taida-lang/os@a.1`,
  `<<<@a.3 @(hello, greet)`, `<<<@a.1 <owner>/<name>` 等の `@a.1` / `@a.3` は
  import / export 構文のサンプル。実在パッケージの版を pin しているわけ
  ではなく、「version 付き import / export が書ける」という構文の例示。
- **error 文字列内の pinned literal** (`docs/reference/addon_manifest.md`):
  error message 自体に literal として焼かれている文字列は surface の
  一部。リファレンスは「この literal が固定されている」事実を pin する
  責務を負う。
- **本 README 自身**: 禁止パターンを列挙する責務上、`<TRACK>B-<NUM>`
  形式の例示や `@<gen>.<num>.rc<n>` のような形式記述、`Round N`,
  `[FIXED]` 等の例示文字列が **本 README ファイル内にのみ** 現れます。
  レビュー時の grep は `--exclude=README.md` を付けるか、本ファイル
  内のヒットは「禁止パターンを教材として使用している箇所」として
  手動で除外してください。

例外を新規追加する場合は、本 README の本節に行を追加して理由を明示
してください。**justified exception 一覧化されない例外は許容されません**。

---

## 4. 書いていいもの (リファレンスの本体)

- **構文** (BNF / 例 / バリエーション)
- **型** (型名 / フィールド / デフォルト値 / モールド対応関係)
- **挙動** (現時点で 3-backend が同じ結果を返す約束、既知の例外)
- **エラー条件** (どの場面でどの `E####` が発射されるか)
- **入出力契約** (返り値 pack の shape、引数の制約、副作用の有無)

挙動の **why** は最小限に留め、**what** と **how to call** に集中します。
why を深掘りしたい場合は `docs/guide/` に置きます。

---

## 5. レビュー時のチェック

新しいリファレンス変更を merge する前に、以下のコマンドで blocker ID /
タグ narrative の混入が無いことを確認してください:

```bash
# 一次走査: 0 件であること (README.md 自身は教材として除外)
# 内部追跡ラベルは `[A-Z][0-9]+B-[0-9]+` 形式、タグナラティブは
# `@[a-z]\.[0-9]+` 形式で一般化したパターンで検出します。
grep -nE "Round [0-9]+|[A-Z][0-9]+B-[0-9]+|@[a-z]\.[0-9]+|FIXED" docs/reference/ --exclude=README.md

# 二次走査: 0 件であること、または明示的に許容された例外のみであること
grep -nE "RC[0-9]+B|RC[0-9]+\.[0-9]+|QF-[0-9]+|2026-" docs/reference/ --exclude=README.md
```

両 sweep が 0 件 (または本 README に列挙された justified exception のみ)
で通過することがリファレンス merge の前提条件です。

---

## 6. CHANGELOG とのリンク

リファレンスから CHANGELOG への参照は、以下のように **タグ単位ではなく
「`CHANGELOG.md` を参照」とだけ書く**のが推奨です:

```markdown
タグ別の land 履歴は `CHANGELOG.md` を参照してください。
```

`@c.26.rc1` のような具体的タグを書くと、rc 番号が変わるたびに
リファレンス側を追従更新する技術負債が発生します。CHANGELOG 側で
タグが変わっても、リファレンスの参照文言は不変であるべきです。
