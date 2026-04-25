# `taida-lang/net` API リファレンス

> Core bundled package. `>>> taida-lang/net => @(...)` で import、または import 無しで直接呼び出し可能（両経路とも checker で同じ型 signature に pin されます）。

3-backend (Interpreter / JS / Native) で parity を保証します。タグ別の land 履歴は `CHANGELOG.md` を参照してください。

---

## 1. 設計方針

Taida の NET surface は **zero-copy span** を基本単位とします:

- `httpServe` handler / `httpParseRequestHead` が返す `req` pack の `method` / `path` / `query` / `headers[i].name` / `headers[i].value` / `body` は **`@(start: Int, len: Int)` の span pack** で、元の `req.raw: Bytes` に対する view です。
- 原本の `Bytes` を clone せず、必要になった時点で user が明示的に **span → Str** または **span-aware 比較** を呼ぶ形にしています (clone-heavy 抑制のため、内部的には `src/interpreter/value.rs` の Arc + try_unwrap COW 共通 abstraction を用います)。
- span pack を受け取る公開 mold 群は `§ 4 span-aware 比較 mold` を参照。
- 「`req.method` を自動で `Str` に昇格する」設計は **採用していません**。span pack を zero-copy の基本単位として永続保持し、ergonomics は span-aware 公開 mold 群 (`§ 4` の `SpanEquals` / `SpanStartsWith` / `SpanContains` / `SpanSlice` / `StrOf`) で解決します。

---

## 2. Server

### 2.1 `httpServe`

```
httpServe(port: Int, handler: Fn, ?opts: BuchiPack) -> Gorillax[@(closed: Bool)]
```

- `port` — bind port。`0` を渡すと OS 割り当て (port は `opts` で返さない、観測には `getsockname` 相当の mold を別途使う想定)。
- `handler` — 下記 2.2 / 2.3 のいずれかの arity を持つ関数値。
- `opts` (optional) — TLS 設定 `@(cert: Str, key: Str, protocol: Str)` 等。`protocol <= "h2"` を指定すると HTTP/2 over TLS。

### 2.2 1-arg handler (response-return form)

```
handler req: BuchiPack = ... => :Str
```

戻り値は HTTP wire 文字列 (`"HTTP/1.1 200 OK\r\n..."`) または `Bytes`。Interpreter / Native で **`src/interpreter/net_eval/h1.rs:1156`** の経路を通ります。

### 2.3 2-arg handler (streaming form)

```
handler req: BuchiPack writer: BuchiPack = ... => :Unit
```

`writer` pack は下記 `writer.write(bytes)` / `writer.end()` などを持つ streaming API。Interpreter / Native で **`src/interpreter/net_eval/h1.rs:840`** の経路を通ります。

> **Important — 2-arg handler body handling**: 2-arg form で handler 内から `req.body` を直接参照すると span の `len` が 0 になります (streaming 前提で body は eagerly 読まれない仕様)。**body を読む場合は必ず `readBody(req)` / `readBodyChunk(req)` / `readBodyAll(req)` のいずれかを使用**してください:
>
> - `readBody(req)` — 1-arg / 2-arg 両対応。2-arg では `readBodyAll` と同等 (stream を全読)。
> - `readBodyChunk(req)` — 2-arg 専用。chunk 単位で `Lax[Bytes]` を返す。残り chunk が無い場合 `Lax` の `hasValue = false`。
> - `readBodyAll(req)` — 2-arg 専用。body を最後まで読んで `Bytes` を返す。
>
> 1-arg handler での `Slice[req.raw, req.body.start, req.body.start + req.body.len]` パスを 2-arg にそのまま持ち込むと **silent に空 Bytes が返る**ため注意。Phase 4 (1-arg) → Phase 5 (2-arg + streaming) の移行で最頻発するハマりどころです。詳細は §3.2 / §8 を参照。

---

## 3. `req` pack shape

`httpServe` handler / `httpParseRequestHead` の返り値 pack は 2 arity で **フィールドの有無が異なります**:

### 3.1 1-arg handler req shape

| Field | Type | 意味 |
|-------|------|------|
| `raw` | `Bytes` | 生 request wire bytes (head + body 全体) |
| `method` | `@(start: Int, len: Int)` | method の span (view over `raw`) |
| `path` | `@(start: Int, len: Int)` | path の span |
| `query` | `@(start: Int, len: Int)` | query の span (query 無しの場合 `len = 0`) |
| `version` | `@(major: Int, minor: Int)` | HTTP version (`major=1 minor=1` / `major=2 minor=0` / ...) |
| `headers` | `@[@(name: span, value: span)]` | header リスト。`name` / `value` 双方が span pack |
| `body` | `@(start: Int, len: Int)` | body の span (`start = bodyOffset`, `len = contentLength` 想定) |
| `bodyOffset` | `Int` | head 終端 offset (= body start) |
| `contentLength` | `Int` | Content-Length header 値 (明示されない場合 `0` または chunked の累積) |
| `remoteHost` | `Str` | peer address の IP string (`"127.0.0.1"` 等) |
| `remotePort` | `Int` | peer port |
| `keepAlive` | `Bool` | HTTP/1.1 default true, `Connection: close` で false |
| `chunked` | `Bool` | `Transfer-Encoding: chunked` の場合 true |

### 3.2 2-arg handler req shape

2-arg form は **streaming** を意図するため、`body` の初期値が 1-arg form と異なります:

| Field | 1-arg handler | 2-arg handler |
|-------|---------------|---------------|
| `body` | span over buffered body (`contentLength` が揃っている) | span `@(start: bodyOffset, len: 0)` (streaming 前提) |
| `contentLength` | 確定値 | header 値をそのまま (>0 でも body は未読) |

他のフィールドは 1-arg と同じ。上記差異は 3-backend で pin されています。

### 3.3 Implementation references

- Interpreter: `src/interpreter/net_eval/h1.rs:1131-1154` (1-arg), `:758-820` (2-arg)
- span pack 構築: `src/interpreter/net_eval/helpers.rs:195-200` (`make_span`, zero-copy)
- Request head parser: `src/interpreter/net_eval/helpers.rs:426-447` (`httpParseRequestHead`)

---

## 4. Span-aware 比較 mold

`taida-lang/net` は span pack を直接受け取る公開 mold 5 種を提供します。3-backend (Interpreter / JS / Native) で parity を保証します。

### 4.0 API summary (full family)

| mold | signature | path | 用途 |
|------|-----------|------|------|
| `SpanEquals` | `SpanEquals[span, raw, needle: Str]() -> Bool` | hot | router の method / path 一致判定 (memcmp 相当、zero allocation) |
| `SpanStartsWith` | `SpanStartsWith[span, raw, prefix: Str]() -> Bool` | hot | router の prefix match (zero allocation) |
| `SpanContains` | `SpanContains[span, raw, needle: Str]() -> Bool` | warm | header 値内の token 検索 (`Accept-Encoding: gzip` 等) |
| `SpanSlice` | `SpanSlice[span, raw, start: Int, end: Int]() -> BuchiPack` | warm | 親 span 内での sub-span 抽出 (zero allocation、`@(start, len)` を返す) |
| `StrOf` | `StrOf[span, raw]() -> Str` / `StrOf(span, raw) -> Str` | cold | span → `Str` 明示 materialize (log 出力 / JSON parse 前のみ使用) |

> **Note on form**: `StrOf` は mold form (`StrOf[...]()`) と function-form (`StrOf(span, raw)`) の両方に対応しています。他の `Span*` family は hot path を意識して mold form 専用です。

以下の public mold を `taida-lang/net` から公開します。3-backend (Interpreter / JS / Native) で parity 保証。

### 4.1 `StrOf[span, raw]() -> Str`

```
m <= StrOf[req.method, req.raw]()     // "GET"
```

span を明示的に `Str` に変換します (PascalCase mold form、Span* family と統一)。**new allocation を発生させる cold path 用**で、ログ出力 / デバッグ / JSON parse 前の materialize に使用します。

- hot path (request ごとの router match 等) では `SpanEquals` / `SpanStartsWith` を使い、materialization を避けてください。
- invalid UTF-8 span / OOB span は **empty `Str`** を返します (tolerant semantics、`Utf8Decode` の Lax と違い直接 Str を返す。cold-path 簡便さを優先)。

等価な既存構文:

```
// 下記も動作 (char-based Slice、cold path の alternative):
m <= Slice[req.raw](start <= req.method.start, end <= req.method.start + req.method.len)
```

> **Implementation note**: Native 実装は `taida_pack_get` + `taida_slice_mold` + `taida_utf8_decode_mold` + `taida_lax_get_or_default` の IR composition (`src/codegen/lower_molds.rs::StrOf`) で、専用の C runtime helper を追加せずに実現しています (core.c / net_h1_h2.c は span-aware mold の追加に対して不変)。既存の `Str[raw](start, end)` 形式は alternative として継続 support します。

### 4.2 `SpanEquals[span, raw, needle: Str]() -> Bool`

```
SpanEquals[req.method, req.raw, "GET"]()   // Bool
```

span が `needle` と byte-level で一致するか判定します。**zero-copy** (memcmp 相当)、router の method 判定に最適化した **hot path 用**。

### 4.3 `SpanStartsWith[span, raw, prefix: Str]() -> Bool`

```
SpanStartsWith[req.path, req.raw, "/api"]()
```

span の先頭が `prefix` か判定。router pattern match 用。

### 4.4 `SpanSlice[span, raw, start: Int, end: Int]() -> BuchiPack`

```
subSpan <= SpanSlice[req.query, req.raw, 0, 10]()   // @(start, len) pack
```

span の sub-span を返します。`start` / `end` は **親 span 内** の offset (0-based)。query の分解 / header value の部分抽出に使用。

### 4.5 `SpanContains[span, raw, needle: Str]() -> Bool`

```
SpanContains[req.headers(0).value, req.raw, "gzip"]()
```

header 値の中に `needle` が含まれるか判定。`Accept-Encoding` 等の値検索に使用。

### 4.6 使い分け指針

| 用途 | 推奨 mold | 理由 |
|------|----------|------|
| router の method 分岐 (per request) | `SpanEquals` | hot path、allocation を避ける |
| router の path prefix 判定 | `SpanStartsWith` | 同上 |
| log 出力 / デバッグ (cold path) | `StrOf` | 1 回だけ allocation、可読性重視 |
| body parsing / JSON 解析 | `StrOf(req.body, req.raw)` を JSON mold に渡す | 一度に allocate して再利用 |
| query string 分解 | `SpanSlice` で分解 → 各 subspan に `StrOf` | 不要な allocation を避ける |

Function-form `StrOf(span, raw)` は §4.1 を参照してください。

---

## 5. HTTP parse / encode

### 5.1 `httpParseRequestHead(bytes: Bytes) -> Lax[BuchiPack]`

request head (start line + header block、CRLFCRLF まで) を parse。返り値の pack shape は §3 とほぼ同じ (`body` / `bodyOffset` / `contentLength` / `remoteHost` / `remotePort` / `keepAlive` / `chunked` は含まない)。

### 5.2 `httpEncodeResponse(status: Int, headers: @[...], body: Bytes) -> Bytes`

response を wire bytes に encode します。

### 5.3 HTTP wire-byte ceilings

`httpServe` / `httpParseRequestHead` は attacker 制御可能な HTTP wire field に **parser 段階で上限**を設け、over-limit 時は `400 Bad Request` を emit してハンドラを呼ばずに接続を閉じます。上限は Native codegen の固定 size stack buffer と揃えてあり、silent truncation を防ぎます。

| field | 上限 | 根拠 |
|-------|------|------|
| method | **16 byte** | `char method[16]` (Native `core.c`) |
| path | **2048 byte** | `char path[2048]` (Native) |
| authority | **256 byte** | `char authority[256]` (Host header) |

> **Implementation note**: Interpreter h1 path は `HTTP_WIRE_MAX_METHOD_LEN = 16` / `HTTP_WIRE_MAX_PATH_LEN = 2048` を `src/interpreter/net_eval/h1.rs` に導入し、`parse_request_head` 後・`dispatch_request` 前で enforcement します。Native / h2 / h3 への enforcement の現況は `CHANGELOG.md` を参照してください。

---

## 6. Client

HTTP client は `taida-lang/os` 側のモールド `HttpRequest[method, url](headers, body)` で提供します (歴史的経緯と OS 境界カテゴリの統一のため)。`taida-lang/net` から re-export はしていません。

### 6.1 `HttpRequest[method, url](headers, body)`

```
HttpRequest[method: Str, url: Str](
  headers: BuchiPack | List[@(name: Str, value: Str)],
  body: Str | Bytes
) -> Async[Lax[@(status: Int, body: Str, headers: BuchiPack)]]
```

- `method` — `"GET"` / `"POST"` / `"PUT"` / `"DELETE"` / `"HEAD"` / `"OPTIONS"` / `"PATCH"`。type-arg 1 番目で指定。
- `url` — type-arg 2 番目。`https://` で始まれば自動で TLS、`http://` で平文。
- `headers` — 2 形式受理 (`§ 6.1.1` 参照)。
- `body` — `Str` / `Bytes`。GET / HEAD で空にする場合は `""`。

戻り値 pack shape:

| Field | Type | 意味 |
|-------|------|------|
| `status` | `Int` | HTTP status code (`200` / `404` / `500` 等) |
| `body` | `Str` | response body (UTF-8 decode 済み)。binary が必要な場合は別 API を使う |
| `headers` | `BuchiPack` | response headers (lower-cased name keys) |

#### 6.1.1 `headers` 引数の 2 形式

| 形式 | 構文 | 制約 |
|------|------|------|
| ぶちパック | `@(content_type <= "application/json")` | フィールド識別子がそのまま wire header 名。`-` や `.` を含むヘッダ名は書けない |
| 名前-値ペアリスト | `@[@(name <= "x-api-key", value <= "secret"), ...]` | 任意の UTF-8 header 名が使える。`-` 含む header 名はこちら必須 |

両形式とも 3 バックエンドで等価。

`HttpRequest["GET"]()` のように type-arg が 2 未満の呼び出しは Interpreter / JS / Native すべてで reject されます。

---

## 7. WebSocket

`taida-lang/net` から export される 5 関数で WebSocket Upgrade / 双方向通信 / クローズを扱います。すべて 2-arg `httpServe` ハンドラの内部からのみ呼び出し可能です。

### 7.1 `wsUpgrade(req, writer)`

```
wsUpgrade(req: BuchiPack, writer: BuchiPack) -> Lax[@(ws: WsToken, ...)]
```

HTTP Upgrade を実行し、以降の WebSocket 通信用 token を返します。**handler の冒頭** (`startResponse` / `writeChunk` / `endResponse` より前) で 1 度だけ呼べます。

- `req` は handler 引数の 1 番目 (request pack)。
- `writer` は handler 引数の 2 番目 (streaming writer)。
- 戻り値の `__value.ws` を以降の `wsSend` / `wsReceive` / `wsClose` に渡します。
- `Sec-WebSocket-Version: 13` (RFC 6455) 固定。GET method 以外、または `Sec-WebSocket-Key` 欠落時は `Lax.failure`。

### 7.2 `wsSend(ws, data)`

```
wsSend(ws: WsToken, data: Str | Bytes) -> Lax[Unit]
```

`Str` を渡すと text frame (`opcode 0x1`)、`Bytes` を渡すと binary frame (`opcode 0x2`) として送出。

### 7.3 `wsReceive(ws)`

```
wsReceive(ws: WsToken) -> Lax[@(type: Str, data: Str | Bytes)]
```

単一 frame を受信。

| `type` | `data` の型 | 意味 |
|--------|------------|------|
| `"text"` | `Str` | UTF-8 text frame |
| `"binary"` | `Bytes` | binary frame |
| `"close"` | `Bytes` | close frame ペイロード (close code を含む。`wsCloseCode` で抽出) |
| `"ping"` | `Bytes` | ping frame ペイロード (応答は wsSend で pong を送る運用、現状自動応答は無し) |
| `"pong"` | `Bytes` | pong frame ペイロード |

### 7.4 `wsClose(ws, ?code)`

```
wsClose(ws: WsToken[, code: Int]) -> Lax[Unit]
```

close frame を送り接続を閉じる。`code` 省略時は `1000` (normal closure)。`1001` (going away) / `1011` (internal error) など RFC 6455 §7.4 の status code を渡せます。

### 7.5 `wsCloseCode(received)`

```
wsCloseCode(received: BuchiPack) -> Lax[Int]
```

`wsReceive` で `type == "close"` の frame を受け取った場合に close code を取り出します。`received` に close 以外の frame を渡すと `Lax.failure`。

---

## 8. Server-Sent Events

### 8.1 `sseEvent(writer, event, data)`

```
sseEvent(writer: BuchiPack, event: Str, data: Str) -> Lax[Int]
```

SSE wire フォーマット (`event:`, `data:`, `\n\n`) を 1 イベント分書き込みます。

- `event` — SSE `event:` フィールド (空文字列で省略)。
- `data` — `data:` フィールド。`\n` を含む場合、SSE 仕様に従って複数の `data:` 行に展開。
- 戻り値は書き込んだ wire bytes 数。

ブラウザ側は `EventSource` API で受信できます。`Content-Type: text/event-stream` の chunk transfer-encoding response として実装されており、`startResponse` を別途呼ぶ必要はありません (1 回目の `sseEvent` で自動送出)。

---

## 9. HttpProtocol enum

`taida-lang/net` から `HttpProtocol` enum が export されます。

```
Enum => HttpProtocol = :H1 :H2 :H3
```

| variant | 意味 |
|---------|------|
| `:H1` | HTTP/1.1 (cleartext or TLS) |
| `:H2` | HTTP/2 over TLS (h2) |
| `:H3` | HTTP/3 over QUIC |

`httpServe` の `opts` 引数で `protocol <= "h2"` のように指定する形式と enum value の両方を受理します。詳しくは `httpServe` の `opts` 仕様を参照してください。

---

## 10. Backend scope

`taida-lang/net` の API surface は **3-backend (Interpreter / JS / Native)** で parity を保証します。WASM バックエンド (`wasm-min` / `wasm-wasi` / `wasm-edge` / `wasm-full`) は `httpServe` / `httpRequest` を提供しません — 該当 capability を呼び出した場合 `[E1612]` を返します。WASM 向け NET dispatcher の現状方針は `docs/STABILITY.md` §1.2 / §4.2 / §5.2 を参照してください。

例外として `readBytesAt` (bytes I/O) の `wasm-wasi` / `wasm-full` lowering のみ widening addition として land 済です。

進行中の blocker や 24 h soak の現況、land 履歴は `CHANGELOG.md` を参照してください。

---

## 11. 2-arg handler body handling patterns

### 11.1 Correct pattern — `readBody` (recommended default)

```taida
>>> taida-lang/net => @(httpServe, readBody, startResponse, endResponse)

handler req writer =
  body <= readBody(req)                 // OK (1-arg / 2-arg 両対応)
  // body is `Bytes`; decode to Str if needed
  bodyStr <= Utf8Decode[body]().getOrDefault("")
  startResponse(writer, 200, @[@(name <= "content-type", value <= "text/plain")])
  endResponse(writer, bodyStr)
=> :Unit
```

### 11.2 Anti-pattern — direct `req.body` span slice (silent breakage)

```taida
// NG — 2-arg form では空 Bytes を返す
handler req writer =
  bodyBytes <= Slice[req.raw, req.body.start, req.body.start + req.body.len]
  // bodyBytes は len=0 の空 Bytes になる (silent breakage)
  ...
=> :Unit
```

この anti-pattern は 1-arg handler で正しく動くため、1-arg → 2-arg 移行時に気づかず残ります。runtime warning 追加の現況は `CHANGELOG.md` を参照してください。

### 11.3 Streaming chunk pattern — `readBodyChunk`

大きな body を chunk ごとに処理する場合は `readBodyChunk`:

```taida
handler req writer =
  // chunk ごとに Lax[Bytes] を返す。hasValue=false で終端
  chunk1 <= readBodyChunk(req)
  chunk1 |
    @(hasValue <= true) <= processChunk(chunk1.value)
    _ <= stdout("EOF")
  ...
=> :Unit
```

### 11.4 Why 2-arg `req.body` span is intentionally empty

2-arg handler は streaming 前提で設計されており、handler 呼び出し時点で body を eagerly 読まない (socket に残したまま)。そのため `req.body` pack は `@(start: bodyOffset, len: 0)` で差し込まれます (1-arg form の `body span = buffered body` とは別 shape)。

`__body_stream` sentinel が内部的に pack に入っており、`readBody*` 系はこの sentinel を検出して socket から直接読み出します。**user 側からこの sentinel を直接触る必要はありません** — `readBody*` のいずれかを呼べば透過的に動作します。

### 11.5 Implementation references

- Interpreter: `src/interpreter/net_eval/mod.rs:118-227` (`readBody` / `readBodyChunk` / `readBodyAll` の 1-arg / 2-arg 分岐)
- `__body_stream` sentinel: `src/interpreter/net_eval/helpers.rs::is_body_stream_request`
- Native: `src/codegen/native_runtime/net_h1_h2.c:721-750` (`taida_net_read_body`)
- JS: `src/js/runtime/net.rs::__taida_net_readBody` (v4: 2-arg body-deferred で `readBodyAll` alias)

### 11.6 3-backend parity tests

- `tests/c26b_023_two_arg_handler_body.rs` — 2-arg handler body handling parity (本 docs と一貫性 pin)
- `tests/parity.rs::parity_http_read_body_*` — Content-Length / empty / chunked の readBody 経路

---

## 12. References

- `docs/STABILITY.md` §2.2 / §5.1 — surface 保証範囲と NET stable viewpoint
- `CHANGELOG.md` — タグ別の land 履歴と blocker 単位の進捗
- `src/interpreter/net_eval/h1.rs` / `h2.rs` — interpreter reference 実装
- `tests/parity.rs::test_net6_*` — 3-backend parity fixtures
