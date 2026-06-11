# `taida-lang/crypto` API リファレンス

`taida-lang/crypto` は暗号系プリミティブを提供するコア同梱パッケージ
です。ハッシュ、HMAC、定数時間比較、hex / base64 変換、暗号論的乱数を
提供します。

```taida
>>> taida-lang/crypto => @(sha256, sha512, sha384, sha224, hmacSha256, constantTimeEquals, hexEncode, hexDecode, base64Encode, base64Decode, randomBytes)
```

`taida-lang/crypto` はプレリュード互換ではなく、利用するには必ず
明示的なインポートが必要です。インポートせずにこれらの関数を呼ぶことは
できません。すべての関数は副作用のないハッシュ／変換か、OS エントロピー
からの乱数取得のいずれかで、`Str` / `Bytes` / `Bool` / `Lax[Bytes]` の
いずれかを返します（値の不在を表す型は返しません）。

---

## 1. ハッシュ関数

### 1.1 `sha256` / `sha512` / `sha384` / `sha224`

> 入力の SHA-2 ダイジェストを小文字 16 進文字列で返す。

```taida fragment
sha256 value: Str => :Str
sha256 value: Bytes => :Str
sha512 value: Str => :Str
sha384 value: Str => :Str
sha224 value: Str => :Str
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `value` | `Str` | UTF-8 バイト列としてハッシュを計算する。 |
| `value` | `Bytes` | バイト列をそのままハッシュする。`Bytes[...]()` は `Lax[Bytes]` を返すので、`>=>` で取り出してから渡す。 |

**Returns**: `:Str` — 小文字 16 進ダイジェスト。大文字は含まない。
出力長はアルゴリズムごとに固定です。

| 関数 | ダイジェスト長 | hex 文字数 | 空入力 (`""`) のダイジェスト |
|------|----------------|-----------|------------------------------|
| `sha256` | 32 バイト | 64 | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| `sha512` | 64 バイト | 128 | `cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e` |
| `sha384` | 48 バイト | 96 | `38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b` |
| `sha224` | 28 バイト | 56 | `d14a028c2a3a2bc9476102bb288234c415a2b01f828ea62ac5b3e42f` |

**Example**:

```taida
sha256("abc") => h256
// h256 = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
sha512("abc") => h512
// h512 = "ddaf35a1...a54ca49f" (128 文字)

Bytes["Hi"]() >=> raw
sha256(raw) => hex_bytes
```

**AI-Context**:
副作用なし。同じ入力からは常に同じ出力を返す純粋関数。`Str` と
`Bytes` の両方を受けるが、それ以外の型は型検査で `[E1506]` として拒否
される。暗黙の文字列化は行わない。`Bytes[...]()` の結果は `Lax[Bytes]`
なので、必ず `>=>` でアンモールドしてから渡すこと。各バックエンドは
1 回の入力を最大 256 MiB まで扱う。

### 1.2 `hmacSha256`

> RFC 2104 の HMAC-SHA256 を小文字 16 進文字列 (64 文字) で返す。

```taida fragment
hmacSha256 key: Str, data: Str => :Str
hmacSha256 key: Bytes, data: Bytes => :Str
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `key` | `Str` \| `Bytes` | HMAC 鍵。ブロックサイズ (64 バイト) を超える鍵は先に SHA-256 でハッシュされる。 |
| `data` | `Str` \| `Bytes` | 認証対象のメッセージ。 |

**Returns**: `:Str` — 64 文字の小文字 16 進。

**Example**:

```taida
hmacSha256("Jefe", "what do ya want for nothing?") => mac
// mac = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
```

**AI-Context**: RFC 4231 のテストベクトルに一致する。`key` と `data` は
独立に処理され、暗黙の型変換は行わない。

---

## 2. 定数時間比較

### 2.1 `constantTimeEquals`

> 2 つのバイト列が等しいかを定数時間で比較する。

```taida fragment
constantTimeEquals a: Str, b: Str => :Bool
constantTimeEquals a: Bytes, b: Bytes => :Bool
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `a` | `Str` \| `Bytes` | 比較対象 1。 |
| `b` | `Str` \| `Bytes` | 比較対象 2。 |

**Returns**: `:Bool` — バイト列が一致すれば `true`。

**AI-Context**: MAC / トークン照合用。実行時間が不一致の位置に依存しない
ように `a` の全長を走査する。長さが異なる場合は `false` を返すが、
長さそのものは秘匿しない（長さの不一致は観測可能）。秘匿が必要な値の
照合に用いること。

**Example**:

```taida
constantTimeEquals(expectedMac, actualMac) => ok
```

---

## 3. エンコード / デコード

### 3.1 `hexEncode` / `base64Encode`

> バイト列を 16 進 / base64 文字列にエンコードする。

```taida fragment
hexEncode data: Str => :Str
hexEncode data: Bytes => :Str
base64Encode data: Str => :Str
base64Encode data: Bytes => :Str
```

**Returns**: `:Str` — `hexEncode` は小文字 16 進。`base64Encode` は
RFC 4648 標準アルファベット (`A–Z a–z 0–9 + /`) + `=` パディング。

**Example**:

```taida
hexEncode("Hi") => h        // h = "4869"
base64Encode("foobar") => b // b = "Zm9vYmFy"
```

### 3.2 `hexDecode` / `base64Decode`

> 16 進 / base64 文字列をバイト列にデコードする。不正入力は失敗側の
> `Lax[Bytes]` を返す。

```taida fragment
hexDecode hex: Str => :Lax[Bytes]
base64Decode b64: Str => :Lax[Bytes]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `hex` / `b64` | `Str` | デコード対象の文字列。`Bytes` ではなく `Str` のみを受ける。 |

**Returns**: `:Lax[Bytes]` — 成功時は `has_value = true` でデコード結果の
`Bytes`。失敗時は `has_value = false` の空 `Lax[Bytes]`。

失敗となる入力:

- `hexDecode`: 奇数長、または 16 進 (`0–9 a–f A–F`) 以外の文字を含む。
- `base64Decode`: 長さが 4 の倍数でない、不正なアルファベット、末尾以外
  の `=`、過剰なパディング。

**Example**:

```taida
hexDecode("4869") >=> raw          // raw = Bytes["Hi"]
hexEncode(raw) => back             // back = "4869"

base64Decode("Zm9vYmFy") >=> bin   // bin = Bytes["foobar"]

hexDecode("zz").has_value => ok    // ok = false（不正 hex）
```

**AI-Context**: デコードの失敗は throw ではなく `Lax[Bytes]` の空側で
表現する（プレリュードの失敗系 API と同じ流儀）。`>=>` でアンモールド
すると成功時はデコード結果、失敗時は空 `Bytes` が得られる。`has_value`
で成否を判定できる。

---

## 4. 乱数

### 4.1 `randomBytes`

> OS の暗号論的乱数源から `n` バイトを取得する。

```taida fragment
randomBytes n: Int => :Bytes
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `n` | `Int` | 取得するバイト数。0 以上。`0` のときは空 `Bytes`。 |

**Returns**: `:Bytes` — `n` バイトの暗号論的乱数。`Lax` ではなく `Bytes`
を直接返すため、`<=` で束縛する（`>=>` の対象ではない）。

**Example**:

```taida
randomBytes(32) => token         // 32 バイトのトークン
hexEncode(token) => tokenHex
```

**エラー**: 負の `n` や OS エントロピー源の取得失敗は throw する（値の
不在を返さない）。`n` の上限は 256 MiB。

**AI-Context**: 各バックエンドの乱数源は次のとおり。

| バックエンド | 乱数源 |
|--------------|--------|
| インタプリタ | OS エントロピー (`/dev/urandom`) |
| ネイティブ | `getentropy(2)`、fallback で `/dev/urandom` |
| 旧 JS ターゲット | `node:crypto` の `randomBytes` |
| `wasm-wasi` / `wasm-full` | WASI `random_get` インポート |
| `wasm-min` / `wasm-edge` | **コンパイル時 reject**（後述） |

---

## 5. バックエンド対応

| バックエンド | 対応範囲 |
|--------------|----------|
| インタプリタ | 全 API 対応 |
| ネイティブ | 全 API 対応 |
| 旧 JS ターゲット | 全 API 対応（ハッシュ系 / HMAC / 乱数は `node:crypto`、hex / base64 / 比較は純 JS） |
| WASM | プロファイル依存（下表） |

### 5.1 WASM プロファイル別対応

| API | `wasm-min` | `wasm-wasi` | `wasm-edge` | `wasm-full` |
|-----|------------|-------------|-------------|-------------|
| `sha256` / `sha512` / `sha384` / `sha224` | 対応 | 対応 | 対応 | 対応 |
| `hmacSha256` | 対応 | 対応 | 対応 | 対応 |
| `constantTimeEquals` | 対応 | 対応 | 対応 | 対応 |
| `hexEncode` / `base64Encode` | 対応 | 対応 | 対応 | 対応 |
| `hexDecode` / `base64Decode` | reject | 対応 | reject | 対応 |
| `randomBytes` | reject | 対応 | reject | 対応 |

ハッシュ系 / `hmacSha256` / `constantTimeEquals` / `hexEncode` /
`base64Encode` は `Str` / `Bool` のみを返すため、guest 内の純粋な実装
として全プロファイルで動作します。host capability bridge は使いません。

`hexDecode` / `base64Decode` / `randomBytes` は `Bytes` を生成するため、
Bytes ランタイムを持つ `wasm-wasi` / `wasm-full` でのみ利用できます。
`randomBytes` は加えて WASI の `random_get` インポートで OS エントロピー
を取得します。`wasm-min` / `wasm-edge` ではこれら 3 つはコンパイル時に
決定的な「does not support」エラーで reject されます。

詳細は
[`docs/reference/wasm_profiles.md`](../reference/wasm_profiles.md) を参照して
ください。
