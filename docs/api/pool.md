# `taida-lang/pool` API リファレンス

`taida-lang/pool` はリソースプーリングの待機セマフォ契約を提供するコア
同梱パッケージです。HTTP クライアントコネクション、DB ドライバ接続、
その他共有資源のプール化を必要とする場面で、取得・返却・破棄・観測の
共通 API を提供します。

```taida
>>> taida-lang/pool => @(poolCreate, poolAcquire, poolRelease, poolClose, poolHealth)
```

プールは **BYO (bring-your-own) リソースモデル** です。リソースの生成
(接続確立など) は呼び出し側の責務で、`poolRelease` で預けた値が次の
`poolAcquire` の再利用リソースとして `Lax` に包まれて返ります。
ドライバごとの接続確立・検証ポリシーは `taida-lang/pool` 自体には
含まれません。

---

## 1. プール生成

### 1.1 `poolCreate`

> プールを生成し、不透明ハンドルを返す。

```taida fragment
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
poolCreate(@(maxSize <= 8, acquireTimeoutMs <= 5000)) >=> created
pool <= created.pool
```

---

## 2. リソース取得

### 2.1 `poolAcquire`

> プールからスロットを 1 つ取得する非同期処理を返す。プールが満杯の
> 場合は空きが出るまで (最大 `acquireTimeoutMs` まで) 待機します。

```taida fragment
poolAcquire pool: Pool => :Async[Result[@(resource: Lax[Resource], token: Token), _]]
poolAcquire pool: Pool  timeoutMs: Int => :Async[Result[@(resource: Lax[Resource], token: Token), _]]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `pool` | `Pool` | `poolCreate` で得た不透明ハンドル。 |
| `timeoutMs` | `Int` | このコール限定の取得待ちタイムアウト (ミリ秒)。省略時は `poolCreate` 時の `acquireTimeoutMs`。明示する場合は `1` 以上の正の整数 (`0` 以下は `kind: "invalid"` の失敗)。 |

**Returns**: `:Async[Result[@(resource: Lax[Resource], token: Token), _]]` —
`>=>` で待機すると `Result` が、もう一度 `>=>` で
`@(resource: Lax[Resource], token: Token)` が得られます。

- `resource` は `Lax` です。アイドルキューの再利用リソース (以前の
  `poolRelease` で預けた値) がある場合は成功側 (`has_value <= true`)、
  新規スロット (fresh) の場合は失敗側 (`has_value <= false`) になります。
  fresh のときはリソース (接続など) を呼び出し側で生成してください。
  失敗側 `Lax` の既定値は実装定義のプレースホルダなので、`>=>` で直接
  アンモールドせず `has_value` 分岐または `getOrDefault` で扱います。
- `token` は `poolRelease` に渡す返却用ハンドルです。

**待機セマンティクス**: プールが満杯 (貸出数が `maxSize` に到達し
アイドルも空) の場合、この `Async` は空きが出るまで実際にブロック
します。待機中に `poolRelease` / `poolClose` が起きれば直ちに進行し、
タイムアウトに達した場合のみ `kind: "timeout"` の失敗になります。

**Failure modes**:

- プールが閉じられている (待機中に閉じられた場合を含む): `kind: "closed"`。
- タイムアウト: `kind: "timeout"`。
- `timeoutMs` が `0` 以下: `kind: "invalid"`。

`Token` / `Resource` は `poolAcquire` 戻り値の `.token` /
`.resource` の中身にそれぞれ対応する型エイリアスです。

**Example**:

```taida fragment
|== error: Error =
  stderr("acquire failed: " + error.message)
=> :Int

poolAcquire(pool, 2000) >=> result    // Async unmold → Result (満杯なら空き待ち)
result >=> acquired                   // Result unmold → @(resource, token)。失敗時 throw
acquired.resource => resLax
conn <= resLax.getOrDefault(makeConnection())   // 再利用 or 新規生成
// conn を使った処理 ...
poolRelease(pool, acquired.token, conn) >=> _
```

---

## 3. リソース返却

### 3.1 `poolRelease`

> 取得済みのリソースをプールに返却する。

```taida fragment
poolRelease pool: Pool  token: Token  resource: Resource => :Result[@(ok: Bool, reused: Bool), _]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `pool` | `Pool` | 取得時と同じプールハンドル。 |
| `token` | `Token` | `poolAcquire` 戻り値の `.token` をそのまま渡す。 |
| `resource` | `Resource` | リソース**本体**。`poolAcquire` が返す `Lax` ラッパーではなく、その中身 (または fresh 時に自作した値) を渡します。預けた値はそのままアイドルキューに入り、次の `poolAcquire` で `Lax` に包まれて返ります (渡した型 `T` が `Lax[T]` で返る対称性)。 |

**Returns**: `:Result[@(ok: Bool, reused: Bool), _]` — `ok` は返却が
成功したか、`reused` は返却したリソースがアイドルキューに再投入されたか
(`true`) 廃棄されたか (`false`) を示します。

**AI-Context**:
返却前にリソースを破棄してしまった場合、別のリソースを渡しても呼び
出しは通りますが、プール側の内部整合性のため新規取得を推奨します。

**Example**:

```taida fragment
// acquired は §2.1 の poolAcquire 成功結果 (@(resource, token)) を想定。
poolRelease(pool, acquired.token, acquired.resource) >=> released
| released.reused |> stdout("再利用キューへ戻った")
| _              |> stdout("リソースは破棄された")
```

---

## 4. プール破棄

### 4.1 `poolClose`

> プールを閉じ、保持しているすべてのアイドルリソースを破棄する。

```taida fragment
poolClose pool: Pool => :Async[Result[@(ok: Bool), _]]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `pool` | `Pool` | 閉じる対象のプールハンドル。 |

**Returns**: `:Async[Result[@(ok: Bool), _]]` — `>=>` で待機し、もう一度
`>=>` で `@(ok: Bool)` を取り出します。進行中の `poolAcquire` 待ちは
失敗 (`kind: "closed"`) で完了します。

**AI-Context**:
同一プールを 2 回以上 `poolClose` しても失敗にはなりません (冪等)。

**Example**:

```taida
poolClose(pool) >=> result    // Async unmold → Result
result >=> closed             // Result unmold → @(ok: Bool)。失敗時 throw
stdout("closed: " + closed.ok.toString())
```

---

## 5. プールの状態取得

### 5.1 `poolHealth`

> プールの現在状態を観測する。

```taida fragment
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
| `waiting` | `Int` | いま空き待ちでブロックしている `poolAcquire` 呼び出しの実数。 |

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
| 旧 JS ターゲット | 全 API |
| WASM (全プロファイル) | 利用不可 |

WASM プロファイルではプール本体のランタイムスレッドモデル (空き待ち
ブロックを成立させる並行実行主体) が利用できないため、`taida-lang/pool`
は利用できません。詳細は
[`docs/reference/wasm_profiles.md`](../reference/wasm_profiles.md) を
参照してください。

---

## 関連リファレンス

- [`README.md`](README.md) — `docs/api/` 全体の入口
- [`docs/api/prelude.md`](prelude.md) — `Result` / `Async` / `Lax` のメソッドとプレリュード関数
- [`docs/guide/11_async.md`](../guide/11_async.md) — `Async[T]` と `>=>` の使い方
