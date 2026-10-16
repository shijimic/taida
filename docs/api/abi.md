# `taida-lang/abi` API リファレンス

`taida-lang/abi` は、外部ホストが Taida 関数をリクエストハンドラとして
呼び出すためのコア同梱パッケージです。HTTP 系ホストやローカル adapter
は、`WebRequest` を渡し、`WebResponse` を受け取ります。

```taida
>>> taida-lang/abi => @(WebRequest, WebResponse, text, json, bytes, status, header)
```

`text` / `json` / `bytes` / `status` / `header` は特別な namespace ではなく、
import された通常のシンボルです。アプリ側に同名の関数や値がある場合は、必要な
type と helper だけを明示 import してください。たとえば JSON helper が不要なら
`json` を import せず、`WebRequest` / `WebResponse` / `text` だけを選びます。

---

## 1. 型

### 1.1 `WebRequest`

```taida
WebRequest = @(
  method: Str,
  path: Str,
  rawQuery: Str,
  query: @[@(name: Str, value: Str)],
  headers: @[@(name: Str, value: Str)],
  body: Bytes
)
```

| Field | Type | Description |
|-------|------|-------------|
| `method` | `Str` | HTTP method。例: `"GET"` / `"POST"`。 |
| `path` | `Str` | path 部分。query string は含めません。 |
| `rawQuery` | `Str` | `?` を除いた raw query string。query が無い場合は空文字列です。 |
| `query` | `@[@(name: Str, value: Str)]` | query parameter の出現順リスト。同名 parameter を複数保持します。 |
| `headers` | `@[@(name: Str, value: Str)]` | request header field line の出現順リスト。同名 header を複数保持します。 |
| `body` | `Bytes` | request body の生 bytes。 |

`query` / `headers` は map ではありません。HTTP や URL の入力では同じ名前が複数回
現れることがあるため、canonical representation は `name` / `value` の
ぶちパックのリストです。lookup 用の派生 helper がある場合も、このリストが
重複と順序を保持する基準です。

### 1.2 `WebResponse`

```taida
WebResponse = @(
  status: Int,
  headers: @[@(name: Str, value: Str)],
  body: Bytes
)
```

| Field | Type | Description |
|-------|------|-------------|
| `status` | `Int` | HTTP status code。未指定時の helper 既定値は `200`。 |
| `headers` | `@[@(name: Str, value: Str)]` | response header field line の出現順リスト。同名 header を複数保持します。 |
| `body` | `Bytes` | response body の生 bytes。 |

---

## 2. 関数

| 関数 | シグネチャ | 概要 |
|------|------------|------|
| `text` | `text body: Str => :WebResponse` | UTF-8 text response。`content-type` は `text/plain; charset=utf-8`。 |
| `json` | `json value => :WebResponse` | `jsonEncode(value)` 相当の JSON response。`content-type` は `application/json`。 |
| `bytes` | `bytes body: Bytes => :WebResponse` | bytes response。`content-type` は `application/octet-stream`。 |
| `status` | `status code: Int  response: WebResponse => :WebResponse` | status code を差し替えた response を返す。範囲外は `100`〜`599` に丸めます。 |
| `header` | `header name: Str  value: Str  response: WebResponse => :WebResponse` | header field line を末尾に追加した response を返す。無効な header 名や改行を含む値は 500 response になります。 |

`status` / `header` は入力 response を変更せず、新しい `WebResponse` を返します。
`header` は既存の同名 field line を削除しません。たとえば `set-cookie` を複数回
追加した場合、`headers` には追加順で複数の field line が残ります。

**Example**:

```taida
>>> taida-lang/abi => @(WebRequest, WebResponse, text, status, header)

handle req: WebRequest =
  header("x-taida", "ok", status(201, text("created: " + req.path)))
=> :WebResponse
```

---

## 3. Host Capability

`taida-lang/abi` は inbound の `WebRequest` / `WebResponse` に加えて、
outbound の host capability 呼び出しを扱う descriptor も提供します。

```taida
>>> taida-lang/abi => @(HostCapability, HostStep, HostCall)
```

| Descriptor | 概要 |
|------------|------|
| `HostCapability[name, kind]()` | adapter が解決する host binding を表す。`name` は binding 名、`kind` は adapter / wrapper ingot が決める opaque な `Str`。 |
| `HostStep[method, args]()` | host 側 method 呼び出しを 1 step 表す。`method` は compile-time `Str`、`args` は wire 可能な値のリスト。 |
| `HostCall[steps, Out]()` | `Cage` の runner として使う host call 記述。`steps` は `HostStep` のリスト、`Out` は decode 後の戻り型。 |

`HostCapability[name, kind]()` は、build target が host capability manifest を
提供する場合に compile-time 照合されます。`name` / `kind` が compile-time
`Str` として解決できない場合、または manifest に未宣言の場合は `[E3603]` です。
kind 文字列の命名規約は `taida-lang/abi` では定義しません。

`HostStep` の `args` は必ずリストです。空引数は `@[]` と書きます。各要素は
`Wired[T]` を満たす必要があります。`Str` / `Int` / `Float` / `Bool` / `Bytes`、
wire 可能な list / ぶちパック / 名前付きぶちパック、`WebRequest` /
`WebResponse`、`HostCapability` が対象です。違反は `[E3601]` です。

`HostCall` は `Cage` で実行します。

```taida
>>> taida-lang/abi => @(WebRequest, WebResponse, text, HostCapability, HostStep, HostCall)

KV <= "cloud/kv"

handle req: WebRequest =
  cache <= HostCapability["CACHE", KV]()
  value <=< Cage[cache, HostCall[@[HostStep["get", @["answer"]]()], Str]()]()
  text(value)
=> :WebResponse
```

戻り値は `Async[Out]` なので、`>=>` / `<=<` で待ちます。host 側の capability
解決失敗、method 不在、host 例外、Promise rejection、`Out` の decode 失敗は
rejected `Async[Out]` になります。通常の `Async` と同じく error ceiling で
受けられます。

```taida
>>> taida-lang/abi => @(WebRequest, WebResponse, text, HostCapability, HostStep, HostCall)

handle req: WebRequest =
  |== error: Error =
    text("host error: " + error.message)
  => :WebResponse

  cache <= HostCapability["CACHE", "cloud/kv"]()
  value <=< Cage[cache, HostCall[@[HostStep["get", @["answer"]]()], Str]()]()
  text(value)
=> :WebResponse
```

### Cage チェーン — `Cage[subject]()` / `InCage` / `Uncage`

step が増えるとネストが深くなる `HostCall` 直接形の代わりに、パイプラインで記述を構築できます。

```taida
>>> taida-lang/abi => @(WebRequest, WebResponse, text, HostCapability)

D1 <= "cloudflare/d1"

handle req: WebRequest =
  db <= HostCapability["DB", D1]()
  Cage[db]() => InCage[_, "prepare", @["select * from posts"]]() => Uncage[_, "all", @[Str]]() >=> rows
  text(rows.first().getOrDefault(""))
=> :WebResponse
```

| モールド | 概要 |
|----------|------|
| `Cage[subject]()` | 1 引数形は **CageBuilder** (capability + 空 steps) を返す。2 引数形 `Cage[subject, runner]()` は従来どおり。 |
| `InCage[builder, method, args]()` | step を 1 つ積んだ builder を返す。`method` は compile-time `Str`、`args` の制約は `HostStep` と同一 (`[E3601]` / `[E3603]`)。 |
| `Uncage[builder, method, Out]()` | 最終 step (引数なし) を積んで envelope 化し、host call を発行して `Async[Out]` を返す。 |

チェーンは**記述の構築**であり、host call は `Uncage` で 1 回だけ発行されます。wire 形 (steps 配列の 1 envelope) は `HostCall` 直接形と同一なので、adapter / glue / fixture は両形で共通です。

builder は一級値で、値セマンティクスを持ちます (`InCage` は元の builder を変更せず拡張コピーを返す)。ベースチェーンを束縛しておけば、軽量な prepared statement として再利用できます。

```taida fragment
db <= HostCapability["DB", D1]()
Cage[db]() => InCage[_, "prepare", @["select * from users where id = ?"]]() => findUser
findUser => InCage[_, "bind", @[7]]() => Uncage[_, "all", @[Str]]() >=> rows
```

builder 自体をアンモールドすることはできません (取り出す値は `Uncage` が返します)。bare な builder への `>=>` / `<=<` はゴリラです。

#### 汎用ヘルパー — Out の型変数透過

`Uncage` の Out にはジェネリック関数の型変数を書けます。クエリごとに HostCall をベタ書きする代わりに、アクセス層を 1 つの汎用関数に畳み込めます:

```taida
queryAll[T] db: CageBuilder  sql: Str =
  db => InCage[_, "prepare", @[sql]]() => Uncage[_, "all", T]() >=> rows
  rows
=> :T

rows <= queryAll[@[Str]](base, "select * from posts")
```

Out スキーマは呼び出しごとの compile-time 値なので、この形の関数は**全ての呼び出しで明示的型引数** (`queryAll[T](...)`) が必要です。推論形の呼び出しは型エラーで拒否されます。

型変数は別の汎用関数へそのまま転送できます (`outer[T] db: CageBuilder  sql: Str = queryAll[T](db, sql) => ...`)。転送はホップ数やモジュール境界 (`>>>` で import した汎用関数への転送) によらず推移的に追跡され、転送元の関数自体も「明示的型引数が必要な関数」になります。

HostCall の wire 値では、`Bytes` は標準 base64 の `Str` として運ばれます。
`WebRequest` / `WebResponse` は handler JSON と同じく body を `bodyBase64`
で運び、`query` / `headers` は `name` / `value` の出現順リストを維持します。

`wasm-edge --handler` は Cloudflare Workers 用の manifest reader を持ち、
`wrangler.jsonc` / `wrangler.json` の binding から host capability manifest を
作ります。対応する binding は D1 database、KV namespace、Durable Object
namespace、R2 bucket、Queue producer、Service binding です。生成される Workers
glue は `env[capability]` から対象を取り、`steps` の method を順に呼ぶ機械的な
bridge です。allow-list 判定、kind 別分岐、payload 検査、result schema decode は
glue では行いません。

### 3.1 Well-known capability: `fetch`

エッジランタイムの outbound HTTP は、binding ではなく常設の well-known
capability `("fetch", "cloudflare/fetch")` として扱います。Workers では
グローバル `fetch` が常に存在するため、`wasm-edge` の manifest reader は
wrangler manifest の有無に関わらずこの capability を manifest に注入します。
生成 glue は capability 名 `fetch` を `env` ではなく `globalThis.fetch` の
bridge に解決します。

step chain は D1 の `prepare`/`bind` と同じ「同種引数のチェーン」です。

```taida
>>> taida-lang/abi => @(WebRequest, WebResponse, text, HostCapability, HostStep, HostCall)

CFFETCH <= "cloudflare/fetch"

handle req: WebRequest =
  fetcher <= HostCapability["fetch", CFFETCH]()
  out: WebRequest <= @(
    method <= "POST",
    path <= "/login/oauth/access_token",
    rawQuery <= "",
    query <= req.query,
    headers <= req.headers,
    body <= req.body
  )
  resp <=< Cage[fetcher, HostCall[@[HostStep["fetch", @["https://github.com/login/oauth/access_token"]](), HostStep["send", @[out]]()], WebResponse]()]()
  text("upstream=" + resp.status.toString())
=> :WebResponse
```

| step | 引数 | 意味 |
|------|------|------|
| `fetch` | `@[url: Str]` | 宛先 URL を確定する。 |
| `send` | `@[req: WebRequest]` | request を送信し、`WebResponse` 形 (`status` / `headers` / `bodyBase64`) で解決する。 |

`send` に渡す `WebRequest` の `path` / `rawQuery` / `query` は outbound では
使われません (URL は `fetch` step の引数が全てです)。`method` / `headers` /
`body` が送信に反映されます。response body は buffered で、redirect は
プラットフォーム既定 (follow) です。host 例外・ネットワーク失敗は通常の
host call 失敗として rejected `Async[Out]` になります。

### 3.2 Well-known capability: `crypto`

`("crypto", "cloudflare/crypto")` も `wasm-edge` の manifest reader が常時
注入する well-known capability です。生成 glue はホストの CSPRNG
(`crypto.getRandomValues`) に解決します。

| step | 引数 | 意味 |
|------|------|------|
| `randomHex` | `@[n: Int]` | n バイトの乱数を小文字 hex `Str` (2n 文字) で解決する。上限 65536 バイト。 |

```taida
cryptoCap <= HostCapability["crypto", "cloudflare/crypto"]()
token <=< Cage[cryptoCap, HostCall[@[HostStep["randomHex", @[16]]()], Str]()]()
```

guest 側 `randomBytes` との使い分け: handler は host call の resume の
たびに先頭から再実行されるため、**suspend をまたいで安定していなければ
ならない乱数** (例: 発行 token — D1 へ書く hash とクライアントへ返す値が
一致する必要がある) は `crypto` capability で取得します。host call の
結果は記録され再実行時にそのまま再生されるためです。suspend をまたがない
乱数は guest 側 `randomBytes` で十分です。

---

## 4. Handler Mode

`taida build native` と各 WASM target は `--handler <SYMBOL>` を受け付けます。
handler 関数は、必ず 1 つの `WebRequest` を受け取り、`WebResponse` を返します。

```bash
taida build native --handler handle app.td -o app-handler
taida build wasm-edge --handler handle app.td -o app.wasm
```

```taida
>>> taida-lang/abi => @(WebRequest, WebResponse, text)

handle req: WebRequest =
  text(req.method + " " + req.path)
=> :WebResponse
```

handler mode は次の条件を満たさない場合、コンパイル時に拒否されます。

| 条件 | 失敗時 |
|------|--------|
| `taida-lang/abi` を import している | `E1961` |
| handler symbol が entry source 内の関数として存在する | `E1961` |
| handler が引数を 1 つだけ持つ | `E1961` |
| 引数型が `WebRequest` | `E1961` |
| 戻り型が `WebResponse` | `E1961` |

Native handler binary は、標準入力から request JSON を 1 件読み、標準出力へ
response JSON を 1 件出力します。WASM handler output は、host adapter が
linear memory の ABI export を呼び出して同じ JSON 形へ変換します。

Request JSON:

```json
{
  "method": "POST",
  "path": "/items",
  "rawQuery": "tag=a&tag=b",
  "query": [
    { "name": "tag", "value": "a" },
    { "name": "tag", "value": "b" }
  ],
  "headers": [
    { "name": "content-type", "value": "text/plain" }
  ],
  "bodyBase64": "aGVsbG8="
}
```

Response JSON:

```json
{
  "status": 200,
  "headers": [
    { "name": "content-type", "value": "text/plain; charset=utf-8" }
  ],
  "bodyBase64": "T0s="
}
```

`bodyBase64` は `Bytes` を JSON 上で安全に運ぶための wire 表現です。Taida
コード内では `body` フィールドは `Bytes` として扱います。

Native / WASM の bridge は、欠落または解釈できない request フィールドに
既定値を入れます。既定値は `method = "GET"`、`path = "/"`、
`rawQuery = ""`、空の `query`、空の `headers`、空の `body` です。

Native / WASM handler bridge は 1 request の wire JSON を最大 16MiB として
扱います。上限を超える request は 413 response に変換されます。

WASM の生成済み Edge glue は、response の `status` をそのまま Fetch の
`Response` に渡します。`204` / `205` / `304` は本文を持てない status なので、
glue は本文を付けずに返します。Fetch の `Response` は `100`〜`199` の
status を表せないため、handler がこの範囲を返した場合、glue は理由を本文に
書いた `500` (`x-taida-error: abi`) を返します。

WASM の生成済み Edge glue は fetch ごとに新しい WebAssembly instance を作り、
request 完了時に破棄します。独自 host で同じ instance を複数 request に再利用
する場合も、`taida_abi_web_free(handle)` を呼ぶとその request の allocator 領域は
巻き戻されます。host は `out_ptr` / `out_len` で response JSON を読み終えてから
必ず `free(handle)` を呼び、`free` 後の pointer を保持しないでください。

WASM handler の `req.body` は `Bytes` surface として扱えます。`bytes(req.body)`
による response body 転送、`req.body.length()`、`req.body.get(i)` は Native /
WASM handler bridge の portable な契約です。

---

## 5. Stable / Provisional Boundary

| 領域 | 状態 | 内容 |
|------|------|------|
| Taida surface | Stable | `WebRequest` / `WebResponse`、response helpers、`HostCapability` / `HostStep` / `HostCall`、`Cage` から返る `Async[Out]`、`Wired[T]` 制約。 |
| Build surface | Stable | `taida build native --handler` と `taida build wasm-* --handler` の entry 検証、handler signature 診断、manifest が active な target での capability 照合。 |
| Handler wire JSON | Stable | Request / response JSON の `method` / `path` / `rawQuery` / `query` / `headers` / `bodyBase64`、および response の `status` / `headers` / `bodyBase64`。 |
| HostCall transport | Provisional | WASM の poll / resume export、`host_call` envelope、generated adapter の内部 loop。adapter author 向けの低レベル transport であり、Taida source surface ではありません。 |
| Adapter detail | Provisional | Cloudflare Workers glue、Wrangler binding reader、各 host runtime での capability 解決方法。 |

将来、低レベル transport は binary ABI や async effect protocol に置き換わる可能性が
あります。その場合でも、Taida source は `Cage[cap, HostCall[...]]()` と
`Async[Out]` の形を維持する方針です。

---

## 6. バックエンド対応

| バックエンド | 対応範囲 |
|--------------|----------|
| インタプリタ | 型・helper 評価と fixture-backed host call に対応。handler binary は生成しません。 |
| ネイティブ | `--handler` で request/response JSON bridge を持つ binary を生成。 |
| WASM (`wasm-min` / `wasm-wasi` / `wasm-edge` / `wasm-full`) | `--handler`、共有 WebRequest/WebResponse ABI、host call poll/resume ABI に対応。 |
| 旧 JS ターゲット | 非対応。`>>> taida-lang/abi` の import 自体が決定的なコンパイル時エラーになります (handler mode・response helper の JS ランタイムは存在しません)。 |
