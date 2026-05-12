# リリースプロセスリファレンス

Taida は semantic versioning を使いません。公開 version は generation と
build counter で表します。

```text
<gen>.<num>[.<label>]
```

stable release で実際に使う tag は release gate で決めます。この文書は、
互換性判断が各 version component にどう対応するかを定義します。

---

## version component

| component | 意味 |
|-----------|------|
| `<gen>` | generation。公開仕様を壊す変更が入ると進みます。 |
| `<num>` | 同一 generation 内の build / iteration。追加変更と bug fix で進みます。 |
| `<label>` | 任意の pre-release label。stable release では省略します。 |

build number は一方向です。pre-release tag が自動的に同じ番号の stable tag へ
昇格することはありません。stable tag は別の release 判断として扱います。

---

## 公開仕様を壊す変更

既存の公開仕様を削除、rename、厳格化する変更、または既存の公開仕様の
観測可能な挙動を変える変更は、公開仕様を壊す変更です。代表例:

- operator の削除または rename
- prelude function、mold、type、CLI command、文書化済み flag の削除または rename
- type signature や manifest schema の厳格化
- public diagnostic code の廃止または renumber
- 文書化済み API の観測可能な意味を変え、以前 valid だった program が
  失敗する、または異なる値を計算するようになる変更

公開仕様を壊す変更は generation bump でのみ land します。その変更を含む
release の前に migration guide が必要です。

---

## 追加変更

以前 valid だった program を変えずに追加できるものは `<num>` increment で land します。
代表例:

- 新しい prelude function や method の追加
- 省略時の挙動を変えない optional manifest field の追加
- 既存 target の意味を変えない accepted target string の追加
- 未使用の public band における新しい diagnostic code の追加

追加変更であっても、release 前に backend parity と documentation coverage を
review します。

---

## bug fix

以前の挙動が文書化済みの bug、または明らかに意図しない挙動だった場合、
bug fix は `<num>` increment で land できます。

一方で、その fix がよく書かれた program を壊し得る観測可能な挙動変更を
含む場合は、公開仕様を壊す変更として扱い、generation bump まで保持します。
判断が曖昧な場合も、公開仕様を壊す変更として扱います。

---

## 非推奨化

公開 symbol、CLI flag、manifest field は同一 generation 内で非推奨にできます。
非推奨化は warning を出しますが、公開仕様からは削除しません。

削除できるのは次の generation bump 以降です。非推奨として残す最短期間は
1 generation です。

---

## 安定仕様の参照先

互換性判断は、以下の公開リファレンスを根拠にします。

- `docs/reference/operators.md`
- `docs/reference/standard_library.md`
- `docs/reference/standard_methods.md`
- `docs/reference/class_like_types.md`
- `docs/reference/cli.md`
- `docs/reference/diagnostic_codes.md`
- `docs/reference/addon_manifest.md`
- `docs/reference/wasm_profiles.md`
- `docs/reference/perf_gates.md`
