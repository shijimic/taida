# `taida-lang/net` API リファレンス

`taida-lang/net` は HTTP / WebSocket / Server-Sent Events を扱うコア同梱
パッケージです。サーバー (`httpServe`)、リクエストパース
(`httpParseRequestHead`)、レスポンスエンコード (`httpEncodeResponse`)、
WebSocket 5 関数、SSE 出力関数を公開します。`Request` の span を扱う
モールド 5 種 (§3) は `taida-lang/net` の export ではなく、import 不要の
組み込みモールドです (本書で仕様を定義します)。

```taida
>>> taida-lang/net => @(httpServe, readBody, startResponse, endResponse, writeChunk, HttpProtocol)
```

`taida-lang/net` はプレリュード非含有のため、利用するには必ず明示的な
インポートが必要です。

HTTP クライアント (`HttpRequest`) は `taida-lang/os` 側で提供されます。
[`docs/api/os.md`](os.md) を参照してください。

---

## 1. サーバー

### 1.1 `httpServe`

> 指定ポートで HTTP サーバーを起動し、各リクエストを `handler` に渡す
> 非同期処理を返す。

```taida
httpServe port: Int  handler: HandlerFn => :Async[Result[@(ok: Bool, requests: Int)]]
httpServe port: Int  handler: HandlerFn  opts: ServeOpts => :Async[Result[@(ok: Bool, requests: Int)]]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `port` | `Int` | bind するポート番号。`0` を渡すと OS が空きポートを割り当てる。 |
| `handler` | `HandlerFn` | リクエスト処理関数。単引数形式 (応答返却) と双引数形式 (ストリーミング) のいずれか。§1.2 / §1.3 を参照。 |
| `opts` | `ServeOpts` | 動作オプション。省略時は全項目デフォルト。§1.4 を参照。 |

**Returns**: `:Async[Result[@(ok: Bool, requests: Int)]]` — `>=>` で待機
すると `Result` が得られ、もう一度 `>=>` で終了結果 pack
`@(ok: Bool, requests: Int)` を取り出します。`ok` は bind / accept ループが
正常に閉じたかどうか、`requests` は実際に処理したリクエスト数です。

**Example**:

```taida
handler req: Request =
  "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello"
=> :Str

httpServe(8080, handler) >=> result    // Async unmold → Result
result >=> summary                     // Result unmold → @(ok, requests)。失敗時 throw
stdout("requests: " + summary.requests.toString())
```

**AI-Context**:
accept ループは終了条件 (`opts.maxRequests` 到達、外部からのキャンセル
等) に達するまで継続します。`port = 0` で起動した場合の実効ポートは
バックエンドの実装ログから観測します。

### 1.2 `HandlerFn` — 単引数ハンドラ (応答返却形式)

```taida
handler req: Request => :Str
handler req: Request => :Bytes
```

リクエスト 1 件あたり HTTP wire 文字列 (`"HTTP/1.1 200 OK\r\n..."`) または
`Bytes` をそのまま return する形式です。`req` の shape は §2.1 を参照。

### 1.3 `HandlerFn` — 双引数ハンドラ (ストリーミング形式)

```taida
handler req: Request  writer: Writer => :Int
```

`writer` 経由でレスポンス本体を逐次書き出す形式です。`writer` は
§1.5 / §1.6 / §1.7 の API (`startResponse` / `writeChunk` / `endResponse`)
を持ちます。戻り値はサーバー側で参照されませんが、関数末尾の I/O 呼び
出し (`endResponse` 等) の `:Int` 戻り値をそのまま伝搬する形が標準
パターンです。Taida は `@()` / `:Unit` を「値の不在」型として認めない
ため、handler も意味のある型 (`:Int`) を返します。

> **重要 — 双引数ハンドラでの body 取得**: 双引数形式は streaming 前提
> で設計されており、ハンドラ呼び出し時点では body を読まずに `req.body`
> の `len` は `0` です。body を読む場合は必ず `readBody(req)` /
> `readBodyChunk(req)` / `readBodyAll(req)` のいずれかを使ってください。
>
> - `readBody(req)` — 単引数 / 双引数いずれにも対応。双引数では body を
>   最後まで読む `readBodyAll` と同等。
> - `readBodyChunk(req)` — 双引数専用。chunk 単位で `Lax[Bytes]` を返し、
>   残り chunk が無くなった時点で `has_value <= false` になる。
> - `readBodyAll(req)` — 双引数専用。body を最後まで読み切って `Bytes` を
>   返す。
>
> 単引数で動く `Slice[req.raw, req.body.start, req.body.start + req.body.len]`
> のような直接 slice を双引数ハンドラへ持ち込むと、`req.body.len` が `0`
> なので **空 `Bytes` が静かに返る**点に注意してください。単引数から双引数
> へ移行する際の典型的な落とし穴です。

### 1.4 `ServeOpts`

`opts` は次のフィールドを持つぶちパックです。

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `maxRequests` | `Int` | `0` (無制限) | この件数を処理した時点で accept ループを終える。 |
| `timeoutMs` | `Int` | バックエンド既定値 | accept / read の待機時間上限 (ミリ秒)。 |
| `maxConnections` | `Int` | バックエンド既定値 | 同時接続数の上限。 |
| `tls` | `TlsOpts` | `@()` (TLS 無効) | TLS 設定。非空にすると HTTPS サーバーとして起動する。 |

`TlsOpts` は以下のぶちパックです。

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `cert` | `Str` | `""` | サーバー証明書 (PEM)。 |
| `key` | `Str` | `""` | 秘密鍵 (PEM)。 |
| `protocol` | `HttpProtocol` | `:H1` | `:H1` で HTTP/1.1、`:H2` で HTTP/2 over TLS、`:H3` で HTTP/3 over QUIC。 |

### 1.5 `startResponse`

> ストリーミングレスポンスの開始行とヘッダを書き出す。

```taida
startResponse writer: Writer  status: Int  headers: @[@(name: Str, value: Str)] => :Int
```

戻り値は実装ごとに有意な `:Int` です。Interpreter / 旧 JS ターゲットでは準備した
start-line + ヘッダ部の合計バイト数を返します (実際の wire 書き込みは
最初の `writeChunk` または `endResponse` 呼び出し時に確定する遅延
コミット方式)。Native は現バージョンでは `0` を返します (具体的な
バイト数の計算は後続バージョンで land)。

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `writer` | `Writer` | 双引数ハンドラの 2 番目の引数。 |
| `status` | `Int` | HTTP ステータスコード (`200`, `404`, `500` 等)。 |
| `headers` | `@[@(name: Str, value: Str)]` | レスポンスヘッダ。要素は `@(name, value)` のぶちパックのみ受理。 |

**Constraints**:

- `headers` は **必ず** `@[@(name: Str, value: Str)]` 形式。他形式は reject。
- `Content-Length` と `Transfer-Encoding` は指定不可 (ランタイムが framing を管理)。
- ヘッダ名は 8192 byte 以下、値は 65536 byte 以下、いずれも CR / LF を含めない。
- ヘッダ名のバイト文法 (RFC 7230 §3.2.6 token) と禁止文字は §3.5 を参照。

### 1.6 `writeChunk`

> ストリーミングレスポンスの body chunk を書き出す。

```taida
writeChunk writer: Writer  data: Str => :Int
writeChunk writer: Writer  data: Bytes => :Int
```

`startResponse` の後、`endResponse` の前に 0 回以上呼び出します。`Str` を
渡した場合は UTF-8 エンコードした byte 列が wire に出ます。戻り値は実装
ごとに有意な `:Int` です。Interpreter / 旧 JS ターゲットでは実際に wire へ書き込んだ
バイト数 (chunked transfer-encoding の hex-size prefix + payload + `\r\n`
suffix を含む合計) を返します。Native は現バージョンでは `0` を返し、
具体的なバイト数の計算は後続バージョンで land します。空 chunk は
no-op で `0` を返します。

書き込みに失敗した場合の挙動はバックエンドごとに異なります。接続が中断
されると実行時エラーが投げられ、現状ではプログラム全体が停止する形で
表面化します (handler 内に明示的な `|==` エラー天井を置けばそこで捕捉
できます)。`docs/api/os.md` の `writeBytes` が `:Result[Int, _]` を
返すのと異なり、net writer 系は失敗側を成功戻り値の `:Int` には混入
させず、別経路 (エラー天井) で扱います。

### 1.7 `endResponse`

> ストリーミングレスポンスを終端する。

```taida
endResponse writer: Writer => :Int
endResponse writer: Writer  trailer: Bytes => :Int
```

最後の chunk を送り、必要に応じて trailing 部分を書き出して接続を閉じ
ます。戻り値は実装ごとに有意な `:Int` です。Interpreter / 旧 JS ターゲットでは終端化
処理で wire に書き込んだバイト数 (chunked terminator `0\r\n\r\n` の 5
バイト + trailer) を返します。Native は現バージョンでは `0` を返します
(具体的なバイト数の計算は後続バージョンで land)。bodyless status (`1xx`
/ `204` / `205` / `304`) では body 部を持たないため `0` を返します。
冪等呼び出し (2 回目以降の `endResponse`) も `0` を返します。

---

## 2. リクエスト / レスポンス型

### 2.1 `Request` (単引数ハンドラ)

| Field | Type | Description |
|-------|------|-------------|
| `raw` | `Bytes` | 生のリクエスト wire bytes (header + body 全体)。 |
| `method` | `@(start: Int, len: Int)` | method の span (`raw` 内の `start` から `len` バイト)。 |
| `path` | `@(start: Int, len: Int)` | path の span。 |
| `query` | `@(start: Int, len: Int)` | query の span。query が無い場合は `len = 0`。 |
| `version` | `@(major: Int, minor: Int)` | HTTP バージョン (`major=1 minor=1` 等)。 |
| `headers` | `@[@(name: span, value: span)]` | ヘッダリスト。`name` / `value` のどちらも span。 |
| `body` | `@(start: Int, len: Int)` | body の span (`start = bodyOffset`、`len = contentLength`)。 |
| `bodyOffset` | `Int` | head 終端 offset (= body 開始位置)。 |
| `contentLength` | `Int` | `Content-Length` ヘッダ値、または chunked の累積バイト数。 |
| `remoteHost` | `Str` | peer の IP 文字列 (`"127.0.0.1"` 等)。 |
| `remotePort` | `Int` | peer のポート番号。 |
| `keepAlive` | `Bool` | HTTP/1.1 既定 `true`、`Connection: close` で `false`。 |
| `chunked` | `Bool` | `Transfer-Encoding: chunked` の場合 `true`。 |

method / path / query / headers の各 span は `Request.raw` 上の view です。
具体的な値が必要な場合は §3 の span モールドを使って取り出します。

### 2.2 `Request` (双引数ハンドラ)

双引数ハンドラは streaming 前提なので、`body` 系フィールドの初期値が
単引数と異なります。

| Field | 単引数ハンドラ | 双引数ハンドラ |
|-------|----------------|----------------|
| `body` | buffered body 全体に対する span | `@(start: bodyOffset, len: 0)` (body は未読) |
| `contentLength` | 確定値 | ヘッダ値そのまま (>0 でも body は未読) |

他のフィールドは単引数と同じです。

---

## 3. span モールド

`Request.method` などの span から具体値を取り出すモールド 5 種の仕様を
定義します。これらは import 不要の組み込みモールドです
(`>>> taida-lang/net` の export リストには含まれません)。

### 3.0 一覧

| Mold | Signature | 用途 |
|------|-----------|------|
| `StrOf` | `StrOf[span, raw]() => :Str` | span を `Str` に取り出す。 |
| `SpanEquals` | `SpanEquals[span, raw, needle: Str]() => :Bool` | span と `needle` が byte 列として一致するか。 |
| `SpanStartsWith` | `SpanStartsWith[span, raw, prefix: Str]() => :Bool` | span の先頭が `prefix` で始まるか。 |
| `SpanContains` | `SpanContains[span, raw, needle: Str]() => :Bool` | span 内に `needle` が含まれるか。 |
| `SpanSlice` | `SpanSlice[span, raw, start: Int, end: Int]() => :@(start: Int, len: Int)` | 親 span 内の部分 span を返す。 |

すべての span モールドの `span` 引数は `@(start: Int, len: Int)` 形式の
ぶちパック、`raw` 引数は `Bytes` を取ります。

### 3.1 `StrOf`

> span を `Str` として取り出す。

```taida
StrOf[span: @(start: Int, len: Int), raw: Bytes]() => :Str
StrOf(span: @(start: Int, len: Int), raw: Bytes) => :Str
```

`StrOf` だけはモールド形式 (`StrOf[...]()`) と関数形式 (`StrOf(...)`) の
両方を受理します。span が invalid UTF-8 や範囲外を指していた場合は空
`Str` を返します。

**Example**:

```taida
m <= StrOf[req.method, req.raw]()
// m = "GET"
```

### 3.2 `SpanEquals`

```taida
SpanEquals[span: @(start: Int, len: Int), raw: Bytes, needle: Str]() => :Bool
```

**Example**:

```taida
isGet <= SpanEquals[req.method, req.raw, "GET"]()
```

### 3.3 `SpanStartsWith`

```taida
SpanStartsWith[span: @(start: Int, len: Int), raw: Bytes, prefix: Str]() => :Bool
```

**Example**:

```taida
isApi <= SpanStartsWith[req.path, req.raw, "/api"]()
```

### 3.4 `SpanContains`

```taida
SpanContains[span: @(start: Int, len: Int), raw: Bytes, needle: Str]() => :Bool
```

**Example**:

```taida
req.headers.get(0) >=> first
hasGzip <= SpanContains[first.value, req.raw, "gzip"]()
```

### 3.5 `SpanSlice`

```taida
SpanSlice[span: @(start: Int, len: Int), raw: Bytes, start: Int, end: Int]() => :@(start: Int, len: Int)
```

親 span 内の `[start, end)` 範囲を指す sub span を返します。`start` /
`end` は親 span 内 0-based。

**Example**:

```taida
subSpan <= SpanSlice[req.query, req.raw, 0, 10]()
```

### 3.6 使い分け

| 用途 | 推奨モールド |
|------|-------------|
| method / path の文字列一致判定 | `SpanEquals` |
| path の prefix 判定 | `SpanStartsWith` |
| ヘッダ値内のトークン検索 | `SpanContains` |
| query string の分解 | `SpanSlice` を併用して各部 span を取り出す |
| ログ出力 / JSON parse 前の取り出し | `StrOf` |

---

## 4. HTTP parse / encode

### 4.1 `httpParseRequestHead`

> リクエストヘッダ部 (start line + ヘッダ block、CRLFCRLF まで) を parse
> する。

```taida
httpParseRequestHead bytes: Bytes => :Result[BuchiPack, _]
```

`Result.Ok` 側の pack shape は §2.1 の `Request` から `body` /
`bodyOffset` / `contentLength` / `remoteHost` / `remotePort` /
`keepAlive` / `chunked` を除き、さらに以下 2 フィールドを加えたもの:

| Field | Type | 意味 |
|-------|------|------|
| `complete` | `Bool` | `bytes` が CRLFCRLF まで揃っているか。`false` の場合は partial parse (続きの bytes を読んで再度呼ぶ) |
| `consumed` | `Int` | head 部の終端 byte offset (`complete <= true` のとき意味あり) |

partial parse は失敗ではなく `complete <= false` の **成功** として
返ります (Ok 側)。`Err` 側に落ちるのは httparse が malformed と判断
したケースのみです。

### 4.2 `httpEncodeResponse`

> ステータス・ヘッダ・body から HTTP wire bytes を組み立てる。

```taida
httpEncodeResponse status: Int  headers: @[@(name: Str, value: Str)]  body: Bytes => :Bytes
```

### 4.3 HTTP wire-byte 上限

`httpServe` / `httpParseRequestHead` は次のフィールドに parser 段階で
上限を設けます。上限超過時はハンドラを呼ばずに `400 Bad Request` を返し
て接続を閉じます。

| Field | 上限 |
|-------|------|
| method | 16 byte |
| path | 2048 byte |
| authority (`Host` ヘッダ) | 256 byte |

### 4.4 chunked transfer-encoding ガード

`Transfer-Encoding: chunked` の `chunk-size` には 3 段のガードがあります。
いずれかに違反するリクエストは `400 Bad Request` ＋ 接続クローズで reject
されます。

1. **桁数上限**: 16 進 16 桁以上の `chunk-size` は parse せずに reject。
   上限は 15 桁 (`FFFFFFFFFFFFFFF`)。leading-zero でも literal byte 数で
   評価されるため、`0000000000000001` (16 桁) は magnitude が 1 でも
   reject。
2. **OWS reject**: `chunk-size` 内・前後の SP / HTAB / CR / LF は RFC 7230
   §4.1 に従いすべて reject。リバースプロキシが OWS を寛容に解釈する一方
   で本実装が strict に reject することで request smuggling 経路を遮断
   します。
3. **算術 overflow ガード**: 15 桁以下の入力でも、累算は 64 bit で
   overflow 検知付きに行い、`SIZE_MAX` を超える値は reject。

空 `chunk-size` (`;ext\r\n` のような行) は streaming / eager どちらの
読み出し path でも reject されます。`;` 以降の chunk-extension 自体は
ignore しますが、`chunk-size` 内に空白を含めることは許しません (RFC 7230
errata 4667 の `BWS ";"` 寛容条項より strict)。

malformed `chunk-size` を検出した場合、`httpServe` は当該接続だけを閉じ、
accept ループは他のクライアントを受け付け続けます。

#### chunk-line / trailer DoS ガード

桁数 / OWS / overflow に加え、chunk-extension flooding と trailer flooding
への hard cap を設けています。

| Guard | 値 | 適用範囲 |
|-------|----|---------|
| chunk-line 最大長 | 1 MiB | `chunk-size` 行 (chunk-size + chunk-extension) と各 trailer 行の最大長 |
| trailer 行数上限 | 64 行 | 終端 chunk (`0\r\n`) 後に許容する trailer header 行数 |
| trailer 合計上限 | 8 KiB | 全 trailer 行の合計バイト数 (終端 CRLF を除く) |

いずれかを超えた framing は `400 Bad Request` ＋ 接続クローズで reject
されます。eager path (単引数ハンドラの `readBody`) と streaming path
(双引数ハンドラの `readBodyChunk` / `readBodyAll`) で同じ cap が掛かり
ます。

`Trailer:` リクエストヘッダに列挙されていない trailer 名の reject
(RFC 9110 §6.5 SHOULD) は現状未実装です。trailer 行数 / 合計上限の二段
で実用上の攻撃面は閉じています。

### 4.5 ヘッダ名 / 値の文法

`startResponse` の `headers` と `httpEncodeResponse` の `headers` は次の
RFC 7230 / 9110 文法を強制します。

**name の文法**: RFC 7230 §3.2.6 token (= 1\*tchar)。`0-9 / A-Z / a-z`
および `! # $ % & ' * + - . ^ _ \` | ~`。それ以外のバイト (NUL、`:`、
SP、HTAB、CR、LF、0x00..0x1F / 0x7F の制御文字、0x80..0xFF の obs-text)
はすべて reject。

**value の文法**: RFC 7230 §3.2 field-value byte (HTAB / SP / VCHAR /
obs-text)。`0x09 / 0x20..0x7E / 0x80..0xFF` のみ許容。NUL、CR、LF、
それ以外の 0x00..0x1F / 0x7F の制御文字はすべて reject。

**name の追加禁止**:

- `_` を含む name は reject。リバースプロキシが `underscores_in_headers`
  設定で正規化を変えるため、攻撃者が `Content_Length: 10` を 1 個追加する
  だけで CL.CL request smuggling が成立する経路を遮断します。
- `Content-Length` / `Transfer-Encoding` / `Set-Cookie` はランタイム
  予約。ハンドラから指定すると reject されます。

---

## 5. WebSocket

`taida-lang/net` の WebSocket API は 5 関数で構成され、すべて双引数
`httpServe` ハンドラ内からのみ呼び出せます。

### 5.1 `wsUpgrade`

> HTTP Upgrade を実行し WebSocket 通信用トークンを返す。

```taida
wsUpgrade req: Request  writer: Writer => :Lax[@(ws: WsConn)]
```

ハンドラの冒頭 (`startResponse` / `writeChunk` / `endResponse` より前) で
1 度だけ呼べます。戻り値を `>=>` でアンモールドして得た `ws` を以降の
`wsSend` / `wsReceive` / `wsClose` に渡します。

`Sec-WebSocket-Version: 13` (RFC 6455) 固定。GET 以外の method、または
`Sec-WebSocket-Key` ヘッダ欠落時は `Lax.failure`。

### 5.2 `wsSend`

> WebSocket フレームを 1 件送出する。

```taida
wsSend ws: WsConn  data: Str => :Int
wsSend ws: WsConn  data: Bytes => :Int
```

`Str` を渡すと text frame (opcode `0x1`)、`Bytes` を渡すと binary frame
(opcode `0x2`) として送出します。戻り値は実装ごとに有意な `:Int` です。
Interpreter / 旧 JS ターゲットでは WebSocket フレームとして wire に書き込んだバイト数
(header + masked payload を含む) を返します。Native は現バージョンでは
`0` を返します (具体的なバイト数の計算は後続バージョンで land)。

### 5.3 `wsReceive`

> WebSocket フレームを 1 件受信する。

```taida
wsReceive ws: WsConn => :Lax[@(type: Str, data: Str | Bytes)]
```

| `type` | `data` の型 | 意味 |
|--------|------------|------|
| `"text"` | `Str` | UTF-8 text frame |
| `"binary"` | `Bytes` | binary frame |

`ping` は同じ payload の `pong` で自動応答し、次の data / close frame まで
読み進めます。`pong` は unsolicited pong として無視します。`close` frame
を受け取った場合は close reply を送って `Lax.failure` を返し、受信 close
code は `wsCloseCode` で取得します。

#### 5.3.1 フレーム検証

`wsReceive` は RFC 6455 のフレーム検証を行います。違反フレームは
close code を付けて接続を閉じます。

- client → server frame は masked 必須。RSV bit 非 0、fragmented frame
  (`FIN = 0`)、unexpected continuation、unknown opcode は protocol error
  として close `1002`。
- payload 長は最大 16 MiB。超過時は close `1002`。
- control frame (`close` / `ping` / `pong`) の payload は最大 125 byte。
  126 byte 以上の close / ping / pong は close `1002`。
- close frame は payload 0 byte、または 2 byte 以上の有効な close code +
  UTF-8 reason のみ受理。1 byte payload、不正 close code、不正 UTF-8
  reason は close `1002`。
- text frame の payload は strict UTF-8 必須。不正 UTF-8 はユーザー
  ハンドラに渡さず close `1007`。

### 5.4 `wsClose`

> WebSocket 接続を閉じる。

```taida
wsClose ws: WsConn => :Int
wsClose ws: WsConn  code: Int => :Int
```

戻り値は実装ごとに有意な `:Int` です。Interpreter / 旧 JS ターゲットでは wire に送出
した close frame のバイト数 (典型的に header 2 byte + close code 2 byte
= 4) を返します。Native は現バージョンでは `0` を返します (具体的な
バイト数の計算は後続バージョンで land)。

- `code` を省略した場合は `1000` (normal closure) として処理されます。
  RFC 6455 §7.4 の `1001` (going away)、`1011` (internal error) などを
  指定できます。
- 受け付ける `code` は `1000`〜`4999`。範囲外は失敗。
- 同じ `ws` に対して `wsClose` を 2 回以上呼んでもエラーにはなりません
  (冪等)。2 回目以降の呼び出しは wire に何も書かないため戻り値は `0`
  になります (「この呼び出しで実際に書いた bytes」という net writer 系
  共通 contract に従う)。

### 5.5 `wsCloseCode`

> 受信した close frame の close code を取り出す。

```taida
wsCloseCode received: BuchiPack => :Int
```

`wsReceive` が `type == "close"` を返した frame 情報を渡すと close code
が得られます。close 以外の frame を渡した場合、または close code が無い
場合は `0` を返します。

---

## 6. Server-Sent Events

### 6.1 `sseEvent`

> SSE wire フォーマット (`event:`, `data:`, `\n\n`) を 1 イベント分書き
> 出す。

```taida
sseEvent writer: Writer  event: Str  data: Str => :Int
```

戻り値は実装ごとに有意な `:Int` です。Interpreter / 旧 JS ターゲットでは実際に書き
込んだバイト数 (`event:` 行 + `data:` 行群 + 末尾 `\n` の chunked
transfer-encoding として送出した総バイト数) を返します。Native は現
バージョンでは `0` を返します (具体的なバイト数の計算は後続バージョン
で land 予定)。net writer 系の他の API (`writeChunk` / `endResponse`)
と同じ約束に従います。書き込みに失敗した場合の挙動は §1.6 と同じく
実行時エラーの経路に流れます (戻り値の `:Int` に失敗状態は混入しません)。

- `event` — SSE の `event:` フィールド。空文字を渡すと省略 (`event:` 行
  を出力しない)。
- `data` — `data:` フィールド。`\n` を含む場合は SSE 仕様に従って複数
  の `data:` 行に展開されます。

ブラウザ側は `EventSource` API で受信できます。`Content-Type:
text/event-stream` の chunked transfer-encoding レスポンスとして実装
されており、`startResponse` を別途呼ぶ必要はありません (1 回目の
`sseEvent` で自動送出)。

---

## 7. `HttpProtocol` enum

```taida
Enum => HttpProtocol = :H1 :H2 :H3
```

| Variant | 意味 |
|---------|------|
| `:H1` | HTTP/1.1 (cleartext または TLS) |
| `:H2` | HTTP/2 over TLS (h2) |
| `:H3` | HTTP/3 over QUIC |

`httpServe` の `opts.tls.protocol` は `HttpProtocol` enum だけを受け取ります。既定値は `:H1` です。

| Variant | Wire protocol | Backend support |
|---------|---------------|-----------------|
| `:H1` | `h1.1` | Interpreter / Native / WASM / legacy JS |
| `:H2` | `h2` | Interpreter / Native |
| `:H3` | `h3` | Native |

旧 JS ターゲットは `:H1` のみ対応します。WASM バックエンドは HTTP サーバー機能が制限されるため、`httpServe` の高度な protocol 指定をコンパイル時エラーとして拒否します。

---

## 8. 双引数ハンドラ body 処理パターン

### 8.1 推奨パターン — `readBody`

```taida
>>> taida-lang/net => @(httpServe, readBody, startResponse, writeChunk, endResponse)

handler req: Request  writer: Writer =
  body <= readBody(req)
  bodyStr <= Utf8Decode[body]().getOrDefault("")
  startResponse(writer, 200, @[@(name <= "content-type", value <= "text/plain")])
  writeChunk(writer, bodyStr)
  endResponse(writer)
=> :Int
```

`endResponse(writer)` の戻り値 `:Int` がそのまま handler の戻り値として
伝搬します。

### 8.2 アンチパターン — `req.body` の直接読み出し

```taida
// NG: 双引数ハンドラでは req.body の span が空 (len = 0) なので、何を経由しても body は取れない
handler req: Request  writer: Writer =
  bodyStr <= StrOf[req.body, req.raw]()
  // bodyStr は "" (req.body.len = 0 のため)
  startResponse(writer, 200, @[@(name <= "content-type", value <= "text/plain")])
  writeChunk(writer, bodyStr)
  endResponse(writer)
=> :Int
```

このアンチパターンは単引数ハンドラ (`req.body` が buffered body 全体の
span を持つ) では正しく動くため、単引数→双引数の移行時に気付かず残り
やすい点に注意してください。

### 8.3 chunk 単位での処理 — `readBodyChunk`

`readBodyChunk(req)` は `Lax[Bytes]` を返し、chunk が尽きると
`has_value <= false` の Lax を返します。実用上は再帰で全 chunk を回す
形になります。

```taida
handler req: Request  writer: Writer =
  startResponse(writer, 200, @[@(name <= "content-type", value <= "text/plain")])
  forwardChunks(req, writer)
  endResponse(writer)
=> :Int

forwardChunks req: Request  writer: Writer =
  readBodyChunk(req) => chunkLax
  | chunkLax.has_value |>
      chunkLax >=> chunk
      writeChunk(writer, chunk)
      forwardChunks(req, writer)
  | _ |> 0
=> :Int
```

`forwardChunks` は再帰的に `writeChunk` を呼び続け、chunk が尽きたら `0`
を返します (`|>` の default arm)。`writeChunk` / `forwardChunks` は
いずれも `:Int` (書き込みバイト数または末尾の `0`) を返すので、handler
全体も `:Int` を返します。

`Transfer-Encoding: chunked` の chunk-size は §4.4 の三段ガードを通った
値だけが採用されます。violation 時は単引数 eager path では
`400 Bad Request`、双引数 streaming path (`readBodyChunk` /
`readBodyAll`) では protocol error として扱われます。

### 8.4 なぜ双引数の `req.body` は空 span か

双引数ハンドラは streaming 前提のため、ハンドラ呼び出し時点では body を
socket 上に残したままにします。そのため `req.body` は
`@(start: bodyOffset, len: 0)` として渡されます (単引数の
「buffered body 全体に対する span」とは形が異なります)。`readBody*` を
呼べば、`req` の内部状態に応じて socket から透過的に読み出されます。

---

## 9. バックエンド対応

| バックエンド | 対応範囲 |
|--------------|----------|
| インタプリタ | 全 API 対応 |
| ネイティブ | 全 API 対応 |
| 旧 JS ターゲット | 全 API 対応 |
| WASM (`wasm-min` / `wasm-edge`) | `httpServe` 利用不可。呼び出した capability は `[E1612]` を返す。 |
| WASM (`wasm-wasi` / `wasm-full`) | plaintext HTTP/1.1 `httpServe` の部分集合のみ対応。TLS / HTTP/2 / HTTP/3 / WebSocket / streaming body は未提供。 |

`wasm-wasi` / `wasm-full` の `httpServe` 部分集合では、guest 側で
bind / listen は行いません。host が `wasi_snapshot_preview1.sock_accept`
を実装している場合は fd 3 の継承済み listener を使い、それ以外の host
では accept 済み TCP 接続を fd 0 / 1 に接続する socket-activation 形式
で 1 リクエストを処理します。

この部分集合では `port` は host 側 listener の選択に使われず、
`timeoutMs` / `maxConnections` はランタイム内で追加制御されません。`tls`
を非空にすると compile-time reject されます。双引数 streaming ハンドラも
compile-time reject されます。リクエストヘッダが 16 KiB を超えてヘッダ
終端に到達しない場合はハンドラを呼ばずに `413 Payload Too Large` を返し
ます。

例外として `readBytesAt` (バイト I/O) は `wasm-wasi` / `wasm-full` 向け
のコード生成がすでに land 済みです。

各 WASM プロファイルとアドオン dispatcher の対応関係は
[`docs/reference/wasm_profiles.md`](../reference/wasm_profiles.md) と
[`docs/reference/addon_manifest.md`](../reference/addon_manifest.md) を
参照してください。

---

## 10. 関連ドキュメント

- [`docs/api/os.md`](os.md) — HTTP クライアント (`HttpRequest`) を含む `taida-lang/os` パッケージ
- [`docs/api/prelude.md`](prelude.md) — `Lax` / `Result` / `Async` のメソッド契約
- [`docs/reference/release_process.md`](../reference/release_process.md) — 公開仕様の保証範囲と互換性判断
- [`docs/reference/wasm_profiles.md`](../reference/wasm_profiles.md) — WASM プロファイル別の対応 API 一覧
