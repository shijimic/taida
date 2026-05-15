# `taida-lang/pool` API リファレンス

`taida-lang/pool` はリソースプーリングの最小契約を提供するコア同梱
パッケージです。HTTP クライアントコネクション、DB ドライバ接続、その他
共有資源のプール化を必要とする場面で、生成・取得・返却・破棄の共通 API
を提供します。

```taida
>>> taida-lang/pool => @(poolCreate, poolAcquire, poolRelease, poolClose, poolHealth)
```

ドライバごとの接続確立・検証ポリシーは `taida-lang/pool` 自体には
含まれません。上位ライブラリでプール化対象 (`Molten` ハンドル) を
具体化する設計です。

---

## 1. プール生成

### 1.1 `poolCreate`

> プールを生成し、不透明ハンドルを返す。

```taida
poolCreate config: PoolConfig => :Result[@(pool: Pool), _]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `config` | `PoolConfig` | プールの動作パラメータ。全フィールド省略可。詳細は本節下部の表を参照。 |

**PoolConfig fields**:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `maxSize` | `Int` | `10` | 同時保持できるリソースの最大数。`1` 以上の正の整数。 |
| `maxIdle` | `Int` | `maxSize` | アイドル保持の上限。`0` 以上で `maxSize` を超えてはいけない。 |
| `acquireTimeoutMs` | `Int` | `30000` | 取得待ちのタイムアウト (ミリ秒)。`1` 以上の正の整数。 |

**Returns**: `:Result[@(pool: Pool), _]` — 成功側の `pool` フィールドは
不透明ハンドル (`Pool` 型) です。後続の `poolAcquire` / `poolHealth` 等に
そのまま渡します。

**Constraints**:

- `maxSize <= 0` を渡すと失敗 `Result` を返す。
- `maxIdle < 0` または `maxIdle > maxSize` を渡すと失敗 `Result` を返す。
- `acquireTimeoutMs <= 0` を渡すと失敗 `Result` を返す。

**Example**:

```taida
// 成功時のみここを通る。Constraints 違反は throw され、|== が無ければ
// ゴリラ天井で終了する。
poolCreate(@(maxSize <= 8, acquireTimeoutMs <= 5000)) ]=> created
pool <= created.pool
```

---

## 2. リソース取得

### 2.1 `poolAcquire`

> プールからリソースを 1 つ取得する非同期処理を返す。

```taida
poolAcquire pool: Pool => :Async[Result[@(resource: Resource, token: Token), _]]
poolAcquire pool: Pool  timeoutMs: Int => :Async[Result[@(resource: Resource, token: Token), _]]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `pool` | `Pool` | `poolCreate` で得た不透明ハンドル。 |
| `timeoutMs` | `Int` | このコール限定の取得待ちタイムアウト (ミリ秒)。省略時は `poolCreate` 時の `acquireTimeoutMs`。明示する場合は `1` 以上の正の整数。 |

**Returns**: `:Async[Result[@(resource: Resource, token: Token), _]]` —
`]=>` で待機すると `Result` が、もう一度 `]=>` で
`@(resource: Resource, token: Token)` が得られます。`resource` は取り出した
リソース本体、`token` は `poolRelease` に渡す返却用ハンドルです。

**Failure modes**:

- プールが閉じられている: 失敗側に `kind: "closed"` が入る。
- タイムアウト: 失敗側に `kind: "timeout"` が入る。

`Token` / `Resource` は `poolAcquire` 戻り値の `.token` / `.resource` に
それぞれ対応する型エイリアスです。

**Example**:

```taida
|== error: Error =
  stderr("acquire failed: " + error.message)
=> :Int

poolAcquire(pool, 2000) ]=> result    // Async unmold → Result
result ]=> acquired                   // Result unmold → @(resource, token)。失敗時 throw
// acquired.resource を使った処理 ...
poolRelease(pool, acquired.token, acquired.resource) ]=> _
```

---

## 3. リソース返却

### 3.1 `poolRelease`

> 取得済みのリソースをプールに返却する。

```taida
poolRelease pool: Pool  token: Token  resource: Resource => :Result[@(ok: Bool, reused: Bool), _]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `pool` | `Pool` | 取得時と同じプールハンドル。 |
| `token` | `Token` | `poolAcquire` 戻り値の `.token` をそのまま渡す。 |
| `resource` | `Resource` | 取得時のリソース本体。 |

**Returns**: `:Result[@(ok: Bool, reused: Bool), _]` — `ok` は返却が
成功したか、`reused` は返却したリソースがアイドルキューに再投入されたか
(`true`) 廃棄されたか (`false`) を示します。

**AI-Context**:
返却前にリソースを破棄してしまった場合、別のリソースを渡しても呼び
出しは通りますが、プール側の内部整合性のため新規取得を推奨します。

**Example**:

```taida
// acquired は §2.1 の poolAcquire 成功結果 (@(resource, token)) を想定。
poolRelease(pool, acquired.token, acquired.resource) ]=> released
| released.reused |> stdout("再利用キューへ戻った")
| _              |> stdout("リソースは破棄された")
```

---

## 4. プール破棄

### 4.1 `poolClose`

> プールを閉じ、保持しているすべてのアイドルリソースを破棄する。

```taida
poolClose pool: Pool => :Async[Result[@(ok: Bool), _]]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `pool` | `Pool` | 閉じる対象のプールハンドル。 |

**Returns**: `:Async[Result[@(ok: Bool), _]]` — `]=>` で待機し、もう一度
`]=>` で `@(ok: Bool)` を取り出します。進行中の `poolAcquire` 待ちは
失敗 (`kind: "closed"`) で完了します。

**AI-Context**:
同一プールを 2 回以上 `poolClose` しても失敗にはなりません (冪等)。

**Example**:

```taida
poolClose(pool) ]=> result    // Async unmold → Result
result ]=> closed             // Result unmold → @(ok: Bool)。失敗時 throw
stdout("closed: " + closed.ok.toString())
```

---

## 5. プールの状態取得

### 5.1 `poolHealth`

> プールの現在状態を観測する。

```taida
poolHealth pool: Pool => :@(open: Bool, idle: Int, inUse: Int, waiting: Int)
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `pool` | `Pool` | 状態を観測するプールハンドル。 |

**Returns**: `:@(open: Bool, idle: Int, inUse: Int, waiting: Int)` —
同期処理で即座にスナップショットを返します。

| Field | Type | Description |
|-------|------|-------------|
| `open` | `Bool` | プールが利用可能か (`poolClose` 後は `false`)。 |
| `idle` | `Int` | アイドルキューに保持されているリソース数。 |
| `inUse` | `Int` | 現在貸し出し中のリソース数。 |
| `waiting` | `Int` | 取得待ちを起こしている呼び出し数。 |

**AI-Context**:
監視 / メトリクス送信のための観測ポイントとしての利用を想定します。
`waiting > 0` が継続する場合は `maxSize` の引き上げや上位レイヤーの
バックプレッシャ設計の見直しが必要です。

**Example**:

```taida
health <= poolHealth(pool)
stdout("idle: " + health.idle.toString() + ", in use: " + health.inUse.toString())
```

---

## 6. バックエンド対応

| バックエンド | 対応範囲 |
|--------------|----------|
| インタプリタ | 全 API |
| ネイティブ | 全 API |
| JS | 全 API |
| WASM (`wasm-min` / `wasm-wasi` / `wasm-edge`) | 利用不可 |
| WASM (`wasm-full`) | 全 API |

`wasm-min` / `wasm-wasi` / `wasm-edge` プロファイルではプール本体の
ランタイムスレッドモデルが利用できないため、インポート自体が拒否
されます。詳細は
[`docs/reference/wasm_profiles.md`](../reference/wasm_profiles.md) を
参照してください。

---

## 関連リファレンス

- [`README.md`](README.md) — `docs/api/` 全体の入口
- [`docs/api/prelude.md`](prelude.md) — `Result` / `Async` / `Lax` のメソッドとプレリュード関数
- [`docs/guide/11_async.md`](../guide/11_async.md) — `Async[T]` と `]=>` の使い方
