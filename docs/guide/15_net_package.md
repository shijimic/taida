# NET パッケージ

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ

HTTP サーバ・WebSocket・Server-Sent Events・HTTP クライアントは
`taida-lang/net` パッケージに集約されています。低レベルの TCP / UDP
ソケットや DNS は `taida-lang/os` 側 (詳しくは [14_os_package.md](14_os_package.md))
にあり、本パッケージは **アプリケーション層 (HTTP / WebSocket / SSE)** に絞った
高水準 surface を提供します。

```taida
>>> taida-lang/net => @(httpServe, sseEvent, endResponse, wsUpgrade, wsSend, wsReceive, wsClose)
```

bundled package のため `taida install` 不要、`>>>` 1 行で全 surface が
使えます。3 バックエンド (Interpreter / JS / Native) で挙動が一致し、
WASM バックエンドは現状 NET surface を提供しません (`docs/STABILITY.md` §1.2
参照、該当呼び出しは `[E1612]` で reject)。

詳細な API シグネチャは [`docs/reference/net_api.md`](../reference/net_api.md)
を参照してください。本ガイドは「使い方を覚えるための入門」を目的とします。

---

## 1. 3 本柱

`taida-lang/net` は 3 つの高水準 primitive を提供します。

| 用途 | 入口 API | 特徴 |
|------|---------|------|
| HTTP サーバ (response-return) | `httpServe(port, handler, ?maxRequests)` (1-arg handler) | handler が単発で wire 文字列 / pack を返す |
| HTTP サーバ (streaming + WS / SSE) | `httpServe(port, handler, ?maxRequests)` (2-arg handler) | `writer` を渡し、chunk / SSE event / WebSocket frame を都度書き込む |
| HTTP クライアント | `HttpRequest[method, url](headers, body)` (`taida-lang/os` のモールド) | 単発リクエスト、`Async[Lax[response pack]]` を返す |

`httpServe` の **handler arity** で挙動が大きく分かれます。1-arg 形は
レスポンス pack を **戻り値として 1 度返す**だけ、2-arg 形は `writer` 引数
を介して **chunk / event / frame を逐次書き込む** モデルです。WebSocket / SSE
は 2-arg 形でしか書けません。

---

## 2. httpServe — 1-arg ハンドラ (response-return form)

最も簡単な書き方は 1-arg ハンドラ。リクエスト毎に `req` 引数を受け取り、
レスポンス pack を戻り値として返します。

```taida
>>> taida-lang/net => @(httpServe)

handler req =
  @(
    status <= 200,
    headers <= @[@(name <= "content-type", value <= "text/plain")],
    body <= "Hello from taida-lang/net!"
  )
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe(8080, handler, 1)
asyncResult ]=> result
stdout(result.__value.ok.toString())
stdout(result.__value.requests.toString())
```

- **`port`** — bind するローカル port。`0` を渡すと OS 割り当て。
- **`handler`** — `req` を受け取り response pack を返す関数値。
- **`maxRequests`** (省略可) — N 回処理して終了する上限。テスト・サンプルで
  「1 リクエストだけ捌いて exit したい」場合に便利。省略すると無制限。

戻り値は `Async[Gorillax[@(ok: Bool, requests: Int, ...)]]` で、`]=>` で
unmold します。`ok` は bind / accept ループが成功したか、`requests` は
実際に処理した件数を返します。

レスポンス pack の `headers` は **名前-値ペアのリスト** (`@[@(name: Str,
value: Str)]`) として渡すのを推奨します。`-` を含むヘッダ名 (`content-type`,
`x-api-key` 等) はぶちパックフィールド名に書けないため、ペアリスト形式が必須です。

---

## 3. HTTP body の読み方 (1-arg vs 2-arg)

2-arg ハンドラに body を渡すと、**`req.body` の span pack は `len = 0`** に
なります (streaming 前提)。**body を読みたいときは必ず `readBody*` 系 API を
通してください** — span を直接 slice して読もうとすると silent に空 Bytes が
返ります。

| API | 1-arg | 2-arg | 用途 |
|-----|------|------|------|
| `readBody(req)` | OK | OK | body を `Bytes` で 1 度に取得 |
| `readBodyChunk(req)` | (使えない) | OK | chunk ごとに `Lax[Bytes]` を返す。`hasValue=false` で終端 |
| `readBodyAll(req)` | (使えない) | OK | 残りすべてを 1 つの `Bytes` で取得 |

```taida
>>> taida-lang/net => @(httpServe, readBody, startResponse, endResponse)

handler req writer =
  body <= readBody(req)
  startResponse(writer, 200, @[@(name <= "content-type", value <= "text/plain")])
  endResponse(writer, "received " + body.length().toString() + " bytes")
=> :Unit

asyncResult <= httpServe(8083, handler, 1)
asyncResult ]=> result
stdout(result.__value.ok.toString())
```

詳細は [`docs/reference/net_api.md` §8](../reference/net_api.md) の
2-arg handler body handling pattern にまとまっています。

---

## 4. chunk response — startResponse / writeChunk / endResponse

レスポンスを chunk transfer-encoding で逐次返したい場合は 2-arg ハンドラと
`startResponse` → `writeChunk*` → `endResponse` の組み合わせを使います。

```taida
>>> taida-lang/net => @(httpServe, startResponse, writeChunk, endResponse)

handler req writer =
  startResponse(writer, 200, @[@(name <= "content-type", value <= "text/plain; charset=utf-8")])
  writeChunk(writer, "first chunk\n")
  writeChunk(writer, "second chunk\n")
  endResponse(writer, "final bytes\n")
=> :Unit

asyncResult <= httpServe(8084, handler, 1)
asyncResult ]=> result
stdout(result.__value.ok.toString())
```

- `startResponse(writer, status, headers)` — status line + header block を
  1 度だけ送出。再呼び出しはエラー。
- `writeChunk(writer, payload)` — body chunk を送出。0 回以上呼べる。
- `endResponse(writer, ?finalPayload)` — chunk transfer-encoding を閉じる。

**順序は固定**: `startResponse → writeChunk* → endResponse`。順序違反は
runtime error になります (`docs/reference/net_api.md` §8 の writer state
machine を参照)。

---

## 5. WebSocket — wsUpgrade / wsSend / wsReceive / wsClose / wsCloseCode

WebSocket は HTTP の Upgrade で wire レベルプロトコルを切り替えます。
`wsUpgrade` を 2-arg ハンドラの **冒頭** で呼び、戻り値の `ws` token を
以降の `wsSend` / `wsReceive` / `wsClose` に渡します。

```taida
>>> taida-lang/net => @(httpServe, wsUpgrade, wsSend, wsReceive, wsClose)

handler req writer =
  upgrade <= wsUpgrade(req, writer)
  ws <= upgrade.__value.ws
  msg <= wsReceive(ws)
  wsSend(ws, msg.__value.data)
  wsClose(ws)
=> :Unit

asyncResult <= httpServe(8082, handler, 1)
asyncResult ]=> result
stdout(result.__value.ok.toString())
```

| API | 用途 | 制約 |
|-----|------|------|
| `wsUpgrade(req, writer)` | HTTP → WS への upgrade。`req.headers` から `Sec-WebSocket-Key` を読み handshake を完了する | 2-arg ハンドラの冒頭、`startResponse` より前にしか呼べない |
| `wsSend(ws, data)` | テキスト / バイナリフレーム送信。`Str` を渡すと text frame、`Bytes` を渡すと binary frame | `wsUpgrade` 完了後のみ |
| `wsReceive(ws)` | 単一 frame を受信。戻り値 `Lax[@(type: Str, data: Bytes \| Str)]`。`type` は `"text"` / `"binary"` / `"close"` / `"ping"` / `"pong"` | 同上 |
| `wsClose(ws[, code])` | close frame を送り接続を閉じる。`code` は WS close code (1000=normal, 1001=going away, 1011=internal error 等) | 同上 |
| `wsCloseCode(received)` | `wsReceive` で `type="close"` を受け取った場合に close code を取り出す | 受信 frame に対してのみ |

エコーサーバの完全例は [`examples/net_ws_echo.td`](../../examples/net_ws_echo.td)
を参照。WebSocket Upgrade の wire 仕様は RFC 6455 で、`taida-lang/net`
の実装は 13 (`Sec-WebSocket-Version`) を pin しています。

---

## 6. Server-Sent Events — sseEvent

Server-Sent Events (SSE) は単方向の text/event-stream で、`sseEvent` 1 関数
で 1 イベントを送出します。WebSocket と違い handshake は不要で、`Content-Type:
text/event-stream` の chunk transfer として実装されます。

```taida
>>> taida-lang/net => @(httpServe, sseEvent, endResponse)

handler req writer =
  sseEvent(writer, "status", "online")
  sseEvent(writer, "tick", "1")
  sseEvent(writer, "tick", "2")
  endResponse(writer)
=> :Int

asyncResult <= httpServe(8081, handler, 1)
asyncResult ]=> result
stdout(result.__value.ok.toString())
```

`sseEvent(writer, eventName, data)` は `event:`, `data:`, `\n\n` の SSE wire
フォーマットを自動で生成します。多行 data も `\n` を含む文字列をそのまま
渡せます (`data:` 行を必要数だけ展開します)。

ブラウザ側からは `EventSource("/events")` で接続するだけ:

```javascript
const es = new EventSource("http://127.0.0.1:8081/events");
es.addEventListener("tick", (ev) => console.log("tick:", ev.data));
es.addEventListener("status", (ev) => console.log("status:", ev.data));
```

実用例は [`examples/net_sse_broadcaster.td`](../../examples/net_sse_broadcaster.td)
を参照。

---

## 7. HTTP クライアント — HttpRequest

HTTP クライアントは `taida-lang/net` ではなく **`taida-lang/os` のモールド**
として提供されます (歴史的経緯と、ファイル I/O / プロセス起動と同じ
"OS 境界" カテゴリに置く設計のため)。

```taida
>>> taida-lang/os => @(argv)

args <= argv()
url <= (
  | args.length() > 0 |> args.first().getOrDefault("https://example.com")
  | _ |> "https://example.com"
)

resp <= HttpRequest["GET", url](
  headers <= @[@(name <= "user-agent", value <= "taida-net-example/1.0")],
  body <= ""
)
resp ]=> result
report <= (
  | result.hasValue |>
      "status=" + result.__value.status.toString() +
      " body_len=" + result.__value.body.length().toString()
  | _ |> "request failed: " + result.__error.message
)
stdout(report)
```

- **メソッド** は `HttpRequest["GET" | "POST" | "PUT" | "DELETE" | "HEAD" | "OPTIONS" | "PATCH", url]()`
  の type-arg 1 番目で指定。
- **URL** は type-arg 2 番目に書きます。`https://` で始まれば自動で TLS、
  `http://` で平文。
- **headers** はぶちパック (`@(name1 <= value1)`) または名前-値ペアリスト
  (`@[@(name <= ..., value <= ...)]`) で渡します。`-` を含むヘッダ名は
  ペアリスト必須。
- **body** は `Str` / `Bytes` を受けます。GET / HEAD で空にする場合は `""`。
- 戻り値は `Async[Lax[@(status: Int, body: Str, headers: BuchiPack)]]`。

完全な使用例は [`examples/net_http_client.td`](../../examples/net_http_client.td)
を参照してください。

---

## 8. HttpProtocol — H1 / H2 / H3 と TLS option

`HttpProtocol` enum は HTTP プロトコルバージョンを表現する型 literal です。
`taida-lang/net` から export されます。

```taida
>>> taida-lang/net => @(HttpProtocol)
```

| variant | 意味 |
|---------|------|
| `:H1` | HTTP/1.1 (cleartext or TLS) |
| `:H2` | HTTP/2 over TLS (h2) |
| `:H3` | HTTP/3 over QUIC |

TLS および h2 / h3 を使う `httpServe` は `opts` 引数で TLS 設定を渡せます:

```taida
opts <= @(
  cert <= certPem,
  key <= keyPem,
  protocol <= "h2"
)
asyncResult <= httpServe(8443, handler, 0, opts)
```

`protocol <= "h2"` を指定すると HTTP/2 over TLS が有効になります。`cert` /
`key` は PEM 文字列で渡し、3 バックエンドで同一の挙動を保証します。
TLS 構成と h2 / h3 サポートの詳細は `docs/reference/net_api.md` の Backend
scope 節を参照してください。

---

## 9. examples 集成

bundled の `examples/` 配下に 3 種類の小さな実用例があります。それぞれ
3 バックエンド (Interpreter / JS / Native) で同一に動作します。

| ファイル | 内容 | 動かし方 |
|---------|------|---------|
| `examples/net_http_hello.td` | 最小 HTTP server (1-arg handler) | `taida examples/net_http_hello.td` → `curl http://127.0.0.1:8080/` |
| `examples/net_ws_echo.td` | WebSocket echo server | `taida examples/net_ws_echo.td` → `websocat ws://127.0.0.1:8082/` |
| `examples/net_sse_broadcaster.td` | SSE broadcaster (3 events) | `taida examples/net_sse_broadcaster.td` → `curl -N http://127.0.0.1:8081/events` |
| `examples/net_http_client.td` | HTTP GET クライアント | `taida examples/net_http_client.td https://example.com` |
| `examples/net_http_parse_encode.td` | wire bytes → request pack ⇄ response wire 変換 | `taida examples/net_http_parse_encode.td` |

すべての example は `maxRequests = 1` を設定しているため、curl / websocat
で 1 リクエスト送ると即座に終了します。プロダクション用途では `maxRequests`
を省略する (= 無制限) か、十分大きな値を指定してください。

---

## 10. backend サポートと注意点

| Backend | サポート | 備考 |
|---------|---------|------|
| Interpreter | ○ | reference 実装。すべての NET surface が動作 |
| JS | ○ | Node.js v18+。`node:net` / `node:tls` / `node:http` / `node:http2` ベース |
| Native | ○ | `src/codegen/native_runtime/net_h1_h2.c` + `net_h3_quic.c`。h2 / TLS 含む |
| WASM (`wasm-min` / `wasm-wasi` / `wasm-edge` / `wasm-full`) | ✗ | `httpServe` / `HttpRequest` の呼び出しは `[E1612]` で reject。WASM 向け NET dispatcher は現状 surface 外 |

**bundled 制約**:

- `taida-lang/net` は core bundled パッケージのため、`taida install` 不要、
  `packages.tdm` の宣言不要、`>>>` 1 行で読み込めます。
- 同名のユーザ作成パッケージで上書きすることはできません (resolver が core
  bundled を優先します)。
- 全関数は `taida-lang/net` の `<<<` で固定 export されており、
  ぶちパックフィールド名 / モールド名 / 関数名のいずれも変更できません
  (stable surface 契約)。

詳細な API シグネチャ・error condition・implementation 参照は
[`docs/reference/net_api.md`](../reference/net_api.md) にあります。
NET surface の哲学的位置付けは [`docs/STABILITY.md` §5.1](../STABILITY.md)
を参照してください。
