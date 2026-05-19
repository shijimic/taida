# `taida-lang/crypto` API リファレンス

`taida-lang/crypto` は暗号系プリミティブを提供するコア同梱パッケージ
です。現行の公開 API は SHA-256 ハッシュのみです。

```taida
>>> taida-lang/crypto => @(sha256)
```

`taida-lang/crypto` はプレリュード互換ではなく、利用するには必ず
明示的なインポートが必要です。インポートせずに `sha256` を呼ぶことは
できません。

---

## 1. 関数

### 1.1 `sha256`

> 入力の SHA-256 ダイジェストを小文字 16 進文字列 (64 文字) で返す。

```taida
sha256 value: Str => :Str
sha256 value: Bytes => :Str
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `value` | `Str` | UTF-8 バイト列としてハッシュを計算する。 |
| `value` | `Bytes` | バイト列をそのままハッシュする。`Bytes["..."]()` / `Bytes[@[Int]]()` は `Lax[Bytes]` を返すので、`>=>` で取り出してから渡す。 |

**Returns**: `:Str` — 常に 64 文字の小文字 16 進ダイジェスト。大文字は
含まない。空入力 (`""` または空 `Bytes`) のハッシュは
`"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"`。

**Example**:

```taida
sha256("hello") => hex
// hex = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"

Bytes["Hi"]() >=> raw
sha256(raw) => hex_bytes
// hex_bytes = "3639efcd08abb273b1619e82e78c29a7df02c1051b1820e99fc395dcaa3326b8"

sha256("") => empty_hex
// empty_hex = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
```

**AI-Context**:
副作用なし。同じ入力からは常に同じ出力を返す純粋関数。`Str` と
`Bytes` の両方を受けるが、暗黙の型変換ではなく実装側でそれぞれを
独立に処理する。`Lax[Bytes]` を直接渡すと意図しない経路に落ちるため、
`Bytes[...]()` の結果は必ず `>=>` でアンモールドしてから渡すこと。

---

## 2. バックエンド対応

| バックエンド | 対応範囲 |
|--------------|----------|
| インタプリタ | 対応 |
| ネイティブ | 対応 |
| JS | 対応 |
| WASM (`wasm-min` / `wasm-wasi` / `wasm-edge`) | 利用不可 |
| WASM (`wasm-full`) | 対応 |

`wasm-min` / `wasm-wasi` / `wasm-edge` プロファイルでは
`taida-lang/crypto` のインポート自体がコンパイル時に拒否されます。
詳細は [`docs/reference/wasm_profiles.md`](../reference/wasm_profiles.md)
を参照してください。
