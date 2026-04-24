# `taida-lang/net` API リファレンス

> Core bundled package. `>>> taida-lang/net => @(...)` で import、または import 無しで直接呼び出し可能（両経路とも checker で同じ型 signature に pin されます）。

ガイドは存在しません（`docs/guide/` 未作成）。3-backend (Interpreter / JS / Native) parity を GA 条件として C26 で仕上げ中です — `.dev/C26_BLOCKERS.md::C26B-001〜C26B-006` + `C26B-026` を参照。

> **C26 Round 3 land status (2026-04-24)**: HTTP/2 parity は 10-case 3-backend pin を達成 (C26B-001、Round 3 / wE)、Native h2 HPACK custom header preserve 済 (C26B-026、Round 2 / wC)、span-aware 比較 mold 全 5 種 (`SpanEquals` / `SpanStartsWith` / `SpanContains` / `SpanSlice` / `StrOf`) も 3-backend land 済 (C26B-016、Round 2 / wD + Round 3 / wH)。残 gating は C26B-002 TLS、C26B-006 retry shim 撤廃、C26B-022 authority/Native/h2/h3 上限、24 h soak 実施。詳細は § 7 table。

---

## 1. 設計方針

Taida の NET surface は **zero-copy span** を基本単位とします:

- `httpServe` handler / `httpParseRequestHead` が返す `req` pack の `method` / `path` / `query` / `headers[i].name` / `headers[i].value` / `body` は **`@(start: Int, len: Int)` の span pack** で、元の `req.raw: Bytes` に対する view です。
- 原本の `Bytes` を clone せず、必要になった時点で user が明示的に **span → Str** または **span-aware 比較** を呼ぶ形にしています。これは C26B-018 / C26B-024 の clone-heavy 抑制方針 (`src/interpreter/value.rs` の Arc + try_unwrap COW 共通 abstraction) と一致する設計です。
- span pack を受け取る公開 mold 群は `§ 4 span-aware 比較 mold` を参照。
- 「`req.method` を自動で `Str` に昇格する」設計 (Option A) は **不採用が最終確定**しています。span pack を zero-copy の基本単位として永続保持し、ergonomics は Option B+ (span-aware 公開 mold 群 = `§ 4` の `SpanEquals` / `SpanStartsWith` / `SpanContains` / `SpanSlice` / `StrOf`) で解決します。C26B-016 Round 2-3 (2026-04-24) で 3-backend land 済。

---

## 2. Server

### 2.1 `httpServe`

```
httpServe(port: Int, handler: Fn, ?opts: BuchiPack) -> Gorillax[@(closed: Bool)]
```

- `port` — bind port。`0` を渡すと OS 割り当て (port は `opts` で返さない、観測には `getsockname` 相当の mold を別途使う想定。C26B-003 で port-bind race 根治の一環として確定予定)。
- `handler` — 下記 2.2 / 2.3 のいずれかの arity を持つ関数値。
- `opts` (optional) — TLS 設定 `@(cert: Str, key: Str, protocol: Str)` 等。`protocol <= "h2"` を指定すると HTTP/2 over TLS (C26B-001)。TLS 細目は C26B-002 で pin 中。

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

> **Important — 2-arg handler body handling (C26B-023)**: 2-arg form で handler 内から `req.body` を直接参照すると span の `len` が 0 になります (streaming 前提で body は eagerly 読まれない仕様)。**body を読む場合は必ず `readBody(req)` / `readBodyChunk(req)` / `readBodyAll(req)` のいずれかを使用**してください:
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

他のフィールドは 1-arg と同じ。C26B-023 で上記差異を 3-backend で pin します。

### 3.3 Implementation references

- Interpreter: `src/interpreter/net_eval/h1.rs:1131-1154` (1-arg), `:758-820` (2-arg)
- span pack 構築: `src/interpreter/net_eval/helpers.rs:195-200` (`make_span`, zero-copy)
- Request head parser: `src/interpreter/net_eval/helpers.rs:426-447` (`httpParseRequestHead`)

---

## 4. Span-aware 比較 mold (C26B-016, Option B+)

> **Status (2026-04-24, Round 3 完了)**: Option B+ の **全 5 mold が 3-backend 実装済**。Round 2 / wD で `SpanEquals` / `SpanStartsWith` / `SpanContains` / `SpanSlice` が land、Round 3 / wH で cold-path materialiser `StrOf` が IR composition (新 C runtime helper なし) で land。Regression guard: `tests/c26b_016_strof_parity.rs`。

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

> **C26 Round 3 (wH) land scope**: Interpreter / JS / Native 3-backend に `StrOf` mold として land (2026-04-24)。Native 実装は `taida_pack_get` + `taida_slice_mold` + `taida_utf8_decode_mold` + `taida_lax_get_or_default` の IR composition (`src/codegen/lower_molds.rs::StrOf`) で、新 C runtime helper 追加なしに実現 (core.c / net_h1_h2.c 不変)。`tests/c26b_016_strof_parity.rs` で 3-backend parity pin。既存の `Str[raw](start, end)` 形式は alternative として継続 support。

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

> **Note (2026-04-24 Round 3 完了)**: 上記 API family は C26B-016 Option B+ で **3-backend land 済**。`SpanEquals` / `SpanStartsWith` / `SpanContains` / `SpanSlice` は Round 2 / wD、`StrOf` は Round 3 / wH 着地。Regression guard は `tests/c26b_016_strof_parity.rs` (StrOf) と wD land の `tests/c26b_016_*_parity.rs` で複合 pin。Function-form `StrOf(span, raw)` は §4.1 参照。

---

## 5. HTTP parse / encode

### 5.1 `httpParseRequestHead(bytes: Bytes) -> Lax[BuchiPack]`

request head (start line + header block、CRLFCRLF まで) を parse。返り値の pack shape は §3 とほぼ同じ (`body` / `bodyOffset` / `contentLength` / `remoteHost` / `remotePort` / `keepAlive` / `chunked` は含まない)。

### 5.2 `httpEncodeResponse(status: Int, headers: @[...], body: Bytes) -> Bytes`

response を wire bytes に encode。詳細は C26B-022 (Step 3 Option B: parser 上限強制) 完了時に追記。

### 5.3 HTTP wire-byte ceilings (C26B-022, Step 3 Option B)

`httpServe` / `httpParseRequestHead` は attacker 制御可能な HTTP wire field に **parser 段階で上限**を設け、over-limit 時は `400 Bad Request` を emit してハンドラを呼ばずに接続を閉じます。上限は Native codegen の固定 size stack buffer と揃えてあり、silent truncation を防ぎます。

| field | 上限 | 根拠 | Status |
|-------|------|------|--------|
| method | **16 byte** | `char method[16]` (Native `core.c`) | Interpreter h1 `[FIXED]` (Round 3 / wE); Native / h2 / h3 **OPEN** |
| path | **2048 byte** | `char path[2048]` (Native) | Interpreter h1 `[FIXED]` (Round 3 / wE); Native / h2 / h3 **OPEN** |
| authority | **256 byte** | `char authority[256]` (Host header) | **OPEN** (Interpreter含め未 land、wJ / wH 予定) |

> **Implementation note (Round 3 / wE)**: Interpreter h1 path で `HTTP_WIRE_MAX_METHOD_LEN = 16` / `HTTP_WIRE_MAX_PATH_LEN = 2048` を `src/interpreter/net_eval/h1.rs` に導入、`parse_request_head` 後・`dispatch_request` 前で enforcement。§ 6.2 additions (widening) 該当、既存 fixture / error 文字列は無変更。Authority 256 は Host header 検出が raw buffer traversal を必要とするため `helpers.rs` 側に land 予定。
>
> **CI gate**: `-Wformat-truncation` を warning-as-error promote する変更は `.github/workflows/ci.yml` 側の C26B-022 Step 3 の一部として Cluster 2 / 6 で扱います。

---

## 6. Client

### 6.1 `httpRequest(url: Str, ?opts: BuchiPack) -> Gorillax[...]`

HTTP client。TLS 自動判定 (`https://` なら TLS)。詳細は C26B-002 FIXED 時に追記。

---

## 7. Known limitations & roadmap

| 項目 | Blocker | Severity | Phase | Status |
|------|---------|----------|-------|--------|
| HTTP/2 parity (3-backend) | C26B-001 | Must Fix | 1 | `[FIXED]` 10-case pin (Round 3 / wE) |
| Native h2 HPACK custom header preserve | C26B-026 | Must Fix | 1 | `[FIXED]` (Round 2 / wC) |
| TLS 構成安定化 | C26B-002 | Must Fix | 2 | OPEN |
| port-bind race 根治 | C26B-003 | **Critical** | 3 | `[FIXED]` (Round 1) |
| throughput regression gate hard-fail | C26B-004 | Must Fix | 4 | `[FIXED]` (Round 2 / wB) |
| scatter-gather 24h soak | C26B-005 | Must Fix | 5 | runbook `[FIXED]` / 24 h run pending |
| HTTP retry shim 撤廃 | C26B-006 | Must Fix | 6 | OPEN (wJ 予定、C26B-003 FIXED 後) |
| HTTP wire parser 上限強制 | C26B-022 | Must Fix | 12 | Interp h1 method+path `[FIXED]` (Round 3 / wE); authority / Native / h2 / h3 OPEN |
| 2-arg body silent breakage — docs | C26B-023 | Must Fix | 12 | docs `[FIXED]` (Round 3 / wH) / runtime warning OPEN |
| span-aware 比較 mold 公開 | C26B-016 | Must Fix | 12 | `[FIXED]` 全 5 mold (Round 2 / wD + Round 3 / wH) |
| 部分適用 closure capture | C26B-017 | Must Fix | 12 | `[FIXED]` (Round 3 / wH) |
| bytes I/O 3-backend + wasm-wasi | C26B-020 | Must Fix | 10 | 柱 1 + 柱 3 `[FIXED]` (Round 1 / Round 3 wI); 柱 2 OPEN |

WASM バックエンドは gen-C では rejected、D28 送り (`docs/STABILITY.md` §1.2 / §4.2 / §5.2、旧 D26 / D27)。例外として C26B-020 柱 3 (`readBytesAt` の `wasm-wasi` / `wasm-full` lowering) のみ § 6.2 widening addition として land 済。

---

## 8. 2-arg handler body handling patterns (C26B-023)

### 8.1 Correct pattern — `readBody` (recommended default)

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

### 8.2 Anti-pattern — direct `req.body` span slice (silent breakage)

```taida
// NG — 2-arg form では空 Bytes を返す
handler req writer =
  bodyBytes <= Slice[req.raw, req.body.start, req.body.start + req.body.len]
  // bodyBytes は len=0 の空 Bytes になる (silent breakage)
  ...
=> :Unit
```

この anti-pattern は 1-arg handler で正しく動くため、1-arg → 2-arg 移行時に気づかず残る。**diagnostic (runtime warning) 追加は C26B-023 FIXED 時に `diagnostic_codes.md` に新 E1xxx として pin 予定**。

### 8.3 Streaming chunk pattern — `readBodyChunk`

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

### 8.4 Why 2-arg `req.body` span is intentionally empty

2-arg handler は streaming 前提で設計されており、handler 呼び出し時点で body を eagerly 読まない (socket に残したまま)。そのため `req.body` pack は `@(start: bodyOffset, len: 0)` で差し込まれます (1-arg form の `body span = buffered body` とは別 shape)。

`__body_stream` sentinel が内部的に pack に入っており、`readBody*` 系はこの sentinel を検出して socket から直接読み出します。**user 側からこの sentinel を直接触る必要はありません** — `readBody*` のいずれかを呼べば透過的に動作します。

### 8.5 Implementation references

- Interpreter: `src/interpreter/net_eval/mod.rs:118-227` (`readBody` / `readBodyChunk` / `readBodyAll` の 1-arg / 2-arg 分岐)
- `__body_stream` sentinel: `src/interpreter/net_eval/helpers.rs::is_body_stream_request`
- Native: `src/codegen/native_runtime/net_h1_h2.c:721-750` (`taida_net_read_body`)
- JS: `src/js/runtime/net.rs::__taida_net_readBody` (v4: 2-arg body-deferred で `readBodyAll` alias)

### 8.6 3-backend parity tests

- 新規 `tests/c26b_023_two_arg_handler_body.rs` (C26B-023 で land、本 docs と一貫性 pin)
- 既存 `tests/parity.rs::parity_http_read_body_*` (Content-Length / empty / chunked の readBody 経路)

---

## 9. References

- `docs/STABILITY.md` §2.2 / §5.1 — surface 保証範囲と NET stable viewpoint
- `.dev/C26_BLOCKERS.md` — 開発中 blocker の live worklist
- `.dev/C26_DESIGN.md` — Phase 0 Design Lock (Option pin 含む)
- `src/interpreter/net_eval/h1.rs` / `h2.rs` — interpreter reference 実装
- `tests/parity.rs::test_net6_*` — 3-backend parity fixtures
