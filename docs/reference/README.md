# リファレンス索引

`docs/reference/` は Taida 言語の **言語リファレンス** を集めた場所です。
本ディレクトリの各ファイルは、現時点での言語仕様・API シグネチャ・
診断コード・CLI コマンドの確定的な記述を提供します。

学習目的でナラティブから読み始めたい場合は [ガイド](../guide/) を参照してください。

---

## ファイル一覧

| ファイル | 役割 |
|----------|------|
| [`addon_manifest.md`](addon_manifest.md) | `native/addon.toml` のスキーマとアドオンバックエンドの方針 |
| [`build_descriptors.md`](build_descriptors.md) | 複数ターゲットを組み合わせるビルド記述子と成果物グラフ |
| [`cli.md`](cli.md) | `taida` CLI のコマンドとフラグの公開仕様 |
| [`diagnostic_codes.md`](diagnostic_codes.md) | 公開診断コードの一覧 |
| [`documentation_comments.md`](documentation_comments.md) | ドキュメントコメントの構文 |
| [`graph_model.md`](graph_model.md) | 構造的内省 (introspection) グラフモデル |
| [`memory_model.md`](memory_model.md) | バックエンド別のメモリ管理戦略とアドオン所有権規約 |
| [`naming_conventions.md`](naming_conventions.md) | 公開命名規則 |
| [`operators.md`](operators.md) | 演算子と文法上の役割 |
| [`perf_gates.md`](perf_gates.md) | リリース品質に関わるパフォーマンス／リソースゲート |
| [`release_process.md`](release_process.md) | 世代番号・ビルド番号と互換性判断のプロセス |
| [`scope_rules.md`](scope_rules.md) | 字句スコープとモジュールスコープの規則 |
| [`tail_recursion.md`](tail_recursion.md) | 末尾呼び出しと再帰の保証 |
| [`type_constraints.md`](type_constraints.md) | ジェネリック関数とモールドの型制約 |
| [`wasm_profiles.md`](wasm_profiles.md) | WASM ターゲットプロファイルと対応範囲 |

パッケージ単位の API リファレンス (プレリュード関数 / ビルトイン型
メソッド / コレクション / `taida-lang/os` / `taida-lang/net` / `taida-lang/build`
等の公開仕様) は [`docs/api/`](../api/) を参照してください。クラスライク型
の概念とモールドの解剖は [`docs/guide/05_mold.md`](../guide/05_mold.md) を
参照してください。

---

## リファレンスの読み方

リファレンスを開いた読者が知りたいのは **「今この言語はどう動くか」** です。
各ドキュメントは次のいずれかを扱います。

- **構文**: BNF 風の記述、例、バリエーション。
- **型**: 型名・フィールド・デフォルト値・モールド対応関係。
- **挙動**: 正式バックエンドが同じ結果を返す約束と既知の例外。
- **エラー条件**: どの場面でどの `E####` 診断コードが発射されるか。
- **入出力契約**: 返り値 pack の shape、引数の制約、副作用の有無。

挙動の **なぜ** を深く知りたい場合は、対応するガイドを参照してください。
リファレンスは結論のみを扱い、land 経緯や開発時の決定プロセスは含み
ません。タグ別のリリース履歴は [`CHANGELOG.md`](../../CHANGELOG.md)
を参照してください。
