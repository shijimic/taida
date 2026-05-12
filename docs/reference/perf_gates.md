# パフォーマンスゲートリファレンス

このリファレンスは、Taida の公開リリース品質に含まれる
パフォーマンス / リソースゲートを定義します。契約対象は、ゲートの構造、
失敗条件、しきい値、検証 fixture の範囲です。どのマシンでも同じ
benchmark 数値が出ることを保証する文書ではありません。

---

## ゲート一覧

| ゲート | workflow | 実行契機 | 失敗条件 |
|--------|----------|----------|----------|
| throughput regression | `bench.yml` | pull request、main push、scheduled run | 最小 sample 数を満たした後、保存済み EWMA 基準値より 10% を超えて遅くなった場合 |
| peak RSS regression | `bench.yml` | pull request、main push、scheduled run | 最小 sample 数を満たした後、保存済み EWMA 基準値より peak RSS が 10% を超えて増えた場合 |
| Valgrind definitely-lost memory | `memory.yml` | pull request、push | `definitely lost` byte が 1 byte でもある場合 |
| interpreter coverage threshold | `coverage.yml` | scheduled run、manual dispatch | `src/interpreter/` の line coverage が 80% 未満、または branch coverage が 70% 未満の場合 |

coverage gate は pull request の高速経路には含めません。計測用 build が通常の
release build より大幅に遅いためです。ただし、この gate が走る場合は
上記しきい値を満たさないと失敗します。

---

## 基準値の扱い

throughput と peak RSS は同じ回帰判定モデルを使います。

- 基準値は 30 samples 以上を持つまで警告扱いです。30 samples に達すると
  失敗条件として扱います。
- EWMA 更新は最大 10 samples の alpha window を使います。
- 許容幅は実行時間と peak RSS のどちらも 10% です。
- throughput の基準値ファイルと peak RSS の基準値ファイルは、互換の schema を
  持たなければなりません。

この基準値仕様は、workflow の編集で gate が黙って弱くならないように固定されています。

---

## fixture の範囲

repository 内の perf-smoke fixture が、ゲート対象の benchmark 範囲を定義します。
fixture の削除、隠れた workflow fallback、失敗すべき gate への
`continue-on-error: true` 追加は、公開リリース品質の変更として扱います。

backend ごとの観測専用 measurement が存在してもかまいません。ただし、
coverage threshold の基準 backend は interpreter です。

---

## Bytes I/O の invariant

Bytes I/O の性能は、zero-copy path を保つことに依存します。

- `readBytesAt(path, offset, len)` は、その API を公開するすべての backend で
  byte buffer の読み取りを提供します。
- WASI lowering は WASI host interface の file open と read sequence を使います。
- `Bytes` cursor slicing は、chunk ごとに buffer 全体を copy せず、共有 buffer の
  挙動を保ちます。
- 大容量 fixture は opt-in でもかまいません。ただし invariant test は、
  buffer 共有の挙動を引き続き検証します。

これらの invariant は workflow gate だけでなく test でも検証します。
