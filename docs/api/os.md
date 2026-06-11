# `taida-lang/os` API リファレンス

`taida-lang/os` はファイル I/O、プロセス起動、環境変数、標準入出力、
非同期 HTTP / TCP / UDP 操作を提供するコア同梱パッケージです。

```taida
>>> taida-lang/os => @(writeFile, run, tcpConnect, HttpRequest)
```

`taida-lang/os` はプレリュード非含有のため、利用するには明示的なインポート
が必要です。例外として `stdin` / `stdinLine` のみプレリュード関数として
import 不要で利用できます。

本ファイルはリファレンスと利用ガイドを兼ねます。公開 API の戻り値型に登場する
`Lax[T]` / `Result[T, _]` / `Gorillax[T]` の意味と `>=>` (アンモールド) 時の挙動は
[`docs/guide/08_error_handling.md`](../guide/08_error_handling.md) と
[`docs/api/prelude.md §8`](prelude.md) を参照してください。

---

## 1. ファイル I/O — 入力

### 1.1 `Read`

> ファイル内容を UTF-8 テキストとして読み込む。

```taida fragment
Read[path: Str]() => :Lax[Str]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `path` | `Str` | 読み込み対象のファイルパス。 |

**Returns**: `:Lax[Str]` — UTF-8 テキスト。ファイル不在、権限不足、
non-UTF-8 等の失敗時は `Lax` のデフォルト (`""`) を返します。BOM は
除去せず、そのまま含めます。

**Constraints**: 1 回の呼び出しで読み込めるファイルサイズの上限は 64 MiB。

### 1.2 `readBytes`

> ファイル内容をバイナリとして読み込む。

```taida fragment
readBytes path: Str => :Lax[Bytes]
```

**Returns**: `:Lax[Bytes]` — バイナリ全体。失敗時は空 `Bytes`。

**Constraints**: 上限 64 MiB。

### 1.3 `readBytesAt`

> ファイルの指定オフセットから指定長を読み込む。

```taida fragment
readBytesAt path: Str  offset: Int  len: Int => :Lax[Bytes]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `path` | `Str` | 対象パス。 |
| `offset` | `Int` | 0 起点のオフセット (byte)。 |
| `len` | `Int` | 読み込む最大 byte 数。1 回呼び出しの上限は 64 MiB。 |

**Returns**: `:Lax[Bytes]` — `offset` から最大 `len` byte を読み出した
`Bytes`。EOF を越えた場合は読み出せた部分のみ返します。`offset` / `len`
が不正な値 (負値など) の場合は `Lax` のデフォルト (空 `Bytes`) を返し
ます。

### 1.4 `ListDir`

> ディレクトリ直下のエントリ名一覧を返す。

```taida fragment
ListDir[path: Str]() => :Lax[@[Str]]
```

**Returns**: `:Lax[@[Str]]` — ファイル / ディレクトリ名のリスト
(親パスは含めない)。

### 1.5 `Stat`

> パスのメタ情報を取得する。

```taida fragment
Stat[path: Str]() => :Lax[@(size: Int, modified: Int, isDir: Bool)]
```

**Returns** の pack:

| Field | Type | Description |
|-------|------|-------------|
| `size` | `Int` | ファイルサイズ (byte)。ディレクトリは 0 または OS 依存値。 |
| `modified` | `Int` | 最終更新時刻 (epoch ミリ秒)。 |
| `isDir` | `Bool` | ディレクトリなら `true`。 |

### 1.6 `Exists`

> パスが存在するかを判定する。

```taida fragment
Exists[path: Str]() => :Result[Bool, _]
```

**Returns**: `:Result[Bool, _]` — symlink は follow します。`>=>` で
アンモールドすると内側の `Bool` が得られます。

**Throws**: `IoError` — 権限不足等の I/O 失敗時のみ throw。通常の
「存在しない」は throw ではなく `Bool` の `false` で表現します。

---

## 2. ファイル I/O — 出力

### 2.1 `writeFile`

> テキストファイルを書き込む (上書き)。

```taida fragment
writeFile path: Str  content: Str => :Result[Int, _]
```

**Returns**: `:Result[Int, _]` — 書き込んだ byte 数。

**Throws**: `IoError`。

**Constraints**: 上書きモード。書き込み先のディレクトリは自動作成
しません。

### 2.2 `writeBytes`

> バイナリファイルを書き込む (上書き)。

```taida fragment
writeBytes path: Str  content: Bytes => :Result[Int, _]
```

**Returns**: `:Result[Int, _]` — 書き込んだ byte 数。

**Throws**: `IoError`。

### 2.3 `appendFile`

> ファイル末尾にテキストを追記する。

```taida fragment
appendFile path: Str  content: Str => :Result[Int, _]
```

**Returns**: `:Result[Int, _]` — 追記した byte 数。

**Throws**: `IoError`。

**Constraints**: ファイルが存在しない場合は新規作成します。

### 2.4 `remove`

> ファイルまたは空ディレクトリを削除する。

```taida fragment
remove path: Str => :Result[Int, _]
```

**Returns**: `:Result[Int, _]` — 削除した件数 (ファイル削除なら 1)。

**Throws**: `IoError`。

### 2.5 `createDir`

> ディレクトリを作成する (`mkdir -p` 相当)。

```taida fragment
createDir path: Str => :Result[Int, _]
```

**Returns**: `:Result[Int, _]` — 0 なら既に存在、1 なら新規作成。

**Throws**: `IoError`。

### 2.6 `rename`

> ファイル / ディレクトリの名前を変更する。

```taida fragment
rename from: Str  to: Str => :Result[@(ok: Bool, code: Int, message: Str), _]
```

**Returns** の pack:

| Field | Type | Description |
|-------|------|-------------|
| `ok` | `Bool` | 成功なら `true`。 |
| `code` | `Int` | POSIX errno。成功時は `0`。 |
| `message` | `Str` | エラーメッセージ。成功時は `""`。 |

**Throws**: `IoError`。

**Constraints**: 同一ファイルシステム内ではアトミック。クロスマウントは
OS 依存。

---

## 3. プロセス起動

### 3.1 `run`

> プログラムを直接 exec し、stdout / stderr を捕捉する。

```taida fragment
run program: Str  args: @[Str] => :Gorillax[@(stdout: Str, stderr: Str, code: Int)]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `program` | `Str` | プログラム名またはパス。 |
| `args` | `@[Str]` | コマンド引数。そのまま `argv[1..]` に渡る。 |

**Returns** の pack:

| Field | Type | Description |
|-------|------|-------------|
| `stdout` | `Str` | 子プロセスの標準出力。 |
| `stderr` | `Str` | 子プロセスの標準エラー出力。 |
| `code` | `Int` | exit code。シグナル終了時は `128 + signal`。 |

**Failure modes** (アンモールドで gorilla 化する条件):

- fork / exec 失敗: `__error` に `IoError` (`kind`, `code` = POSIX errno)
- 非ゼロ exit: `__error` に `ProcessError` (`code`, `stdout`, `stderr`)

**Constraints**: `execvp` 直呼び出しのためシェル展開・パイプ・リダイレクト
は使えません。

### 3.2 `execShell`

> シェル経由でコマンドを実行し、stdout / stderr を捕捉する。

```taida fragment
execShell command: Str => :Gorillax[@(stdout: Str, stderr: Str, code: Int)]
```

**Constraints**: POSIX 環境では `sh -c <command>`、Windows では
`cmd /C <command>` でラップ。シェル展開・パイプ・リダイレクトが使える
反面、`command` に外部入力を連結するとシェルインジェクションのリスクが
あります。

**Failure modes**: `run` と同じ。

### 3.3 `runInteractive`

> プログラムを直接 exec し、親の標準入出力を継承させる。

```taida fragment
runInteractive program: Str  args: @[Str] => :Gorillax[@(code: Int)]
```

**Returns** の pack は `@(code: Int)` のみ。`stdout` / `stderr` フィールド
は含まれず、アクセスすると `[E1602]` で reject されます。`__value` 等の
内部フィールドへの直接アクセスは `[E1960]` で reject されます。

**Failure modes**:

- fork / exec 失敗: `IoError`
- 非ゼロ exit: `ProcessError` (`code` のみ)
- シグナル終了: `code = 128 + signal` (POSIX)、Windows は best-effort

### 3.4 `execShellInteractive`

> シェル経由でコマンドを実行し、親の標準入出力を継承させる。

```taida fragment
execShellInteractive command: Str => :Gorillax[@(code: Int)]
```

`runInteractive` と同じ戻り型・失敗モード。シェル展開の制約は `execShell`
と同じ。

---

## 4. 環境変数・引数

### 4.1 `EnvVar`

> 環境変数を取得する。

```taida fragment
EnvVar[name: Str]() => :Lax[Str]
```

**Returns**: `:Lax[Str]` — 未設定時は `Lax` のデフォルト (`""`)。

### 4.2 `allEnv`

> 全環境変数のスナップショットを返す。

```taida fragment
allEnv() => :HashMap[Str, Str]
```

**Returns**: `:HashMap[Str, Str]` — 呼び出し時点のスナップショット。
以降の `setEnv` 等は反映されません。

### 4.3 `argv`

> ユーザー引数を取得する。

```taida fragment
argv() => :@[Str]
```

**Returns**: `:@[Str]` — `taida script.td -- a b` の `--` 以降に渡された
要素。`taida` 自身のオプションは含みません。

---

## 5. 標準入力 (プレリュード関数)

`stdin` / `stdinLine` は import 不要で利用できます (`>>> taida-lang/os` は
不要)。

### 5.1 `stdin`

> 標準入力から 1 行を行編集モードで読み取る。

```taida fragment
stdin prompt: Str => :Str
```

**Returns**: `:Str` — 読み取った行 (改行を含まない)。EOF / I/O エラー
時は `""` を返します (失敗は検知できません)。`prompt` を `""` にすると
プロンプトを表示しません。ASCII 入力やパイプ経由の取り込み向けです。

### 5.2 `stdinLine`

> 標準入力から 1 行を UTF-8 対応ライン編集で読み取る。

```taida fragment
stdinLine prompt: Str => :Async[Lax[Str]]
```

**Returns**: `:Async[Lax[Str]]` — `>=>` で待機し、もう一度 `>=>` で
`Lax[Str]` を取り出します。EOF / Ctrl-C / Ctrl-D 等で `Lax.failure("")`
になります。UTF-8 を正しく扱い、矢印キー等のライン編集に対応します。

**Example**:

```taida
stdinLine("お名前: ") >=> line
stdout("こんにちは、" + line.getOrDefault("旅人"))
```

---

## 6. 非同期入力モールド

### 6.1 `ReadAsync`

> ファイル内容を非同期に読み込む。

```taida fragment
ReadAsync[path: Str]() => :Async[Lax[Str]]
```

### 6.2 `HttpGet`

> HTTP GET を発行し、レスポンス body を文字列で取得する。

```taida fragment
HttpGet[url: Str]() => :Async[Lax[Str]]
```

### 6.3 `HttpPost`

> HTTP POST を発行し、レスポンス body を文字列で取得する。

```taida fragment
HttpPost[url: Str, body: Str]() => :Async[Lax[Str]]
```

### 6.4 `HttpRequest`

> 任意メソッド・任意ヘッダで HTTP リクエストを発行する。

```taida fragment
HttpRequest[method: Str, url: Str](headers <= Headers, body <= Str) => :Async[Lax[@(status: Int, body: Str, headers: BuchiPack)]]
```

`Headers` は次のいずれかの形式を取ります。

| 形式 | 例 | 備考 |
|------|-----|------|
| ぶちパック | `@(content_type <= "application/json")` | フィールド識別子がそのまま HTTP wire 上のヘッダ名になります。`-` や `.` を含むヘッダ名 (`x-api-key` 等) は書けません。 |
| 名前-値ペアリスト | `@[@(name <= "x-api-key", value <= "secret"), @(name <= "anthropic-version", value <= "2023-06-01")]` | `@[@(name: Str, value: Str)]`。任意 UTF-8 ヘッダ名が使えます。 |

`-` を含むヘッダ名を扱う場合はペアリスト形式が必須です。

**Returns** の pack:

| Field | Type | Description |
|-------|------|-------------|
| `status` | `Int` | HTTP ステータスコード (`200`, `404` 等)。 |
| `body` | `Str` | レスポンス body (UTF-8 デコード済み)。バイナリが必要な場合は別 API を使う。 |
| `headers` | `BuchiPack` | レスポンスヘッダ (キーは小文字化済み)。 |

**Constraints**: `HttpRequest["GET"]()` のように type-arg が 2 未満の
呼び出しは reject されます (旧 JS ターゲットは `taida build js` 時点で
アリティエラー)。

---

## 7. 非同期ソケット

各関数は非同期で、戻り値は `Async[Result[T, _]]` 形式です。`>=>` で
2 段アンモールドして内側の値を取り出します。`timeoutMs` を末尾に渡すと
タイムアウトを指定できます (省略時はバックエンド既定値)。

すべての関数の **Throws** は `IoError` (POSIX errno 付き) です。

### 7.1 `tcpConnect`

```taida fragment
tcpConnect host: Str  port: Int => :Async[Result[TcpSocket, _]]
tcpConnect host: Str  port: Int  timeoutMs: Int => :Async[Result[TcpSocket, _]]
```

TCP クライアント接続を確立します。

### 7.2 `tcpListen`

```taida fragment
tcpListen port: Int => :Async[Result[TcpListener, _]]
tcpListen port: Int  timeoutMs: Int => :Async[Result[TcpListener, _]]
```

TCP サーバーを bind して listen 状態にします。

### 7.3 `tcpAccept`

```taida fragment
tcpAccept listener: TcpListener => :Async[Result[TcpSocket, _]]
tcpAccept listener: TcpListener  timeoutMs: Int => :Async[Result[TcpSocket, _]]
```

接続を 1 件 accept します。

### 7.4 `socketSend`

```taida fragment
socketSend socket: TcpSocket  data: Str => :Async[Result[Int, _]]
socketSend socket: TcpSocket  data: Str  timeoutMs: Int => :Async[Result[Int, _]]
```

best-effort で送信し、送信できた byte 数を返します。partial send になる
可能性があります。

### 7.5 `socketSendAll`

```taida fragment
socketSendAll socket: TcpSocket  data: Str => :Async[Result[@(ok: Bool, bytesSent: Int), _]]
socketSendAll socket: TcpSocket  data: Str  timeoutMs: Int => :Async[Result[@(ok: Bool, bytesSent: Int), _]]
```

全 byte の送信完了までブロックします。途中でエラーになった場合も
`bytesSent` で「どこまで送れたか」が分かります。

### 7.6 `socketRecv`

```taida fragment
socketRecv socket: TcpSocket => :Async[Result[Str, _]]
socketRecv socket: TcpSocket  timeoutMs: Int => :Async[Result[Str, _]]
```

best-effort 受信。socket バッファに到着している分だけ返します。

### 7.7 `socketSendBytes`

```taida fragment
socketSendBytes socket: TcpSocket  data: Bytes => :Async[Result[Int, _]]
socketSendBytes socket: TcpSocket  data: Bytes  timeoutMs: Int => :Async[Result[Int, _]]
```

バイナリの best-effort 送信。

### 7.8 `socketRecvBytes`

```taida fragment
socketRecvBytes socket: TcpSocket => :Async[Result[Bytes, _]]
socketRecvBytes socket: TcpSocket  timeoutMs: Int => :Async[Result[Bytes, _]]
```

バイナリの best-effort 受信。

### 7.9 `socketRecvExact`

```taida fragment
socketRecvExact socket: TcpSocket  size: Int => :Async[Result[Bytes, _]]
socketRecvExact socket: TcpSocket  size: Int  timeoutMs: Int => :Async[Result[Bytes, _]]
```

`size` byte が揃うまで待機して受信します。

### 7.10 `udpBind`

```taida fragment
udpBind host: Str  port: Int => :Async[Result[UdpSocket, _]]
udpBind host: Str  port: Int  timeoutMs: Int => :Async[Result[UdpSocket, _]]
```

UDP ソケットを bind します。

### 7.11 `udpSendTo`

```taida fragment
udpSendTo socket: UdpSocket  host: Str  port: Int  data: Str => :Async[Result[Int, _]]
udpSendTo socket: UdpSocket  host: Str  port: Int  data: Str  timeoutMs: Int => :Async[Result[Int, _]]
```

UDP データグラムを送信します。戻り値は送信できた byte 数。

### 7.12 `udpRecvFrom`

```taida fragment
udpRecvFrom socket: UdpSocket => :Async[Result[@(data: Str, host: Str, port: Int), _]]
udpRecvFrom socket: UdpSocket  timeoutMs: Int => :Async[Result[@(data: Str, host: Str, port: Int), _]]
```

UDP データグラムを受信します。送信元の `host` / `port` も pack で返ります。

### 7.13 `socketClose`

```taida fragment
socketClose socket: TcpSocket => :Async[Result[@(ok: Bool, code: Int, message: Str), _]]
```

`tcpConnect` / `tcpAccept` が返した TCP ソケットを閉じます。

### 7.14 `listenerClose`

```taida fragment
listenerClose listener: TcpListener => :Async[Result[@(ok: Bool, code: Int, message: Str), _]]
```

`tcpListen` が返した listener を閉じます。`tcpAccept` で受理済みの
ソケットは個別に `socketClose` する必要があります。

### 7.15 `udpClose`

```taida fragment
udpClose socket: UdpSocket => :Async[Result[@(ok: Bool, code: Int, message: Str), _]]
```

`udpBind` が返した UDP ソケットを閉じます。型が異なるため `socketClose`
とは呼び分ける必要があります。型不整合は `[E1602]` で reject されます。

---

## 8. エラー型

### 8.1 `IoError`

```taida
IoError = @(
  message: Str,
  kind: Str,
  code: Int,
)
```

- `kind` — `"not_found"` / `"permission_denied"` / `"already_exists"` 等。
- `code` — POSIX errno (`ENOENT = 2`, `EACCES = 13` 等)。

### 8.2 `ProcessError`

`run` / `execShell` (captured 版):

```taida
ProcessError = @(
  code: Int,
  stdout: Str,
  stderr: Str,
)
```

`runInteractive` / `execShellInteractive` (interactive 版):

```taida
ProcessError = @(
  code: Int,
)
```

---

## 9. バックエンド対応

| バックエンド | 対応範囲 |
|--------------|----------|
| インタプリタ | 全 API |
| ネイティブ | 全 API |
| 旧 JS ターゲット | 全 API |
| WASM (`wasm-min`) | 利用不可 |
| WASM (`wasm-wasi` / `wasm-full`) | 文書化済みの WASI 向け部分集合 (`EnvVar` / `allEnv` / `Read` / `Exists` / `writeFile` / `readBytesAt`) |
| WASM (`wasm-edge`) | `EnvVar` / `allEnv` のみ |

WASM プロファイル別の詳細は
[`docs/reference/wasm_profiles.md`](../reference/wasm_profiles.md) と
[`docs/api/build_descriptors.md`](build_descriptors.md) のターゲット別
コア API 互換性表を参照してください。

---

## 10. パス境界ポリシー (import / module loader)

`>>> ./X.td` / `>>> ../Y.td` / `>>> /absolute/Y.td` のような filesystem
import は **プロジェクトルート** を境界とします。プロジェクトルートの
マーカーは `packages.tdm` / `taida.toml` / `.git/` のいずれかです。
`.taida/` は依存・ビルド出力・ユーザーキャッシュの置き場であり、
プロジェクトルートマーカーには含めません。`~/.taida/` だけで `$HOME`
全体をプロジェクトルートとして扱うこともしません。

マーカーが見つからない standalone なソースでは、そのソースが置かれて
いるディレクトリだけを fallback の境界として扱います。正式バックエンド
すべてで同一の境界判定とエラーメッセージを返します。

| パターン | 例 | 結果 |
|---------|-----|------|
| ルート内に閉じた `..` | `>>> ./sub/../sibling.td` | 許容 (解決後がルート内のため) |
| ルート内に閉じた nested `..` | `>>> ./sub/nested/../../sibling.td` | 許容 |
| ルート外への relative traversal | `>>> ./../outside.td` | reject |
| ルート外への absolute path | `>>> /tmp/outside/file.td` | reject |

reject 時のエラーメッセージは正式バックエンドで完全一致します。

```
Import path '<exact import token>' resolves outside the project root. Path traversal beyond the project boundary is not allowed.
```

`<exact import token>` には import 元コードに書かれた文字列がそのまま
入ります (`./../outside.td` であればそのリテラル、`/tmp/outside/file.td`
であればそのリテラル)。lexical-vs-resolved の区別はなく、wire 上は
入力トークンを保ったまま fail-fast します。

この境界判定はインストール時のパッケージストアパス検証とは別の検査
領域であり、両者は重なりません。

---

## 11. 関連ドキュメント

- [`docs/api/net.md`](net.md) — HTTP サーバー / WebSocket / SSE を扱う `taida-lang/net` パッケージ
- [`docs/api/prelude.md`](prelude.md) — `Result[T, P]` / `Lax[T]` / `Gorillax[T]` のメソッドと型コンストラクタ
- [`docs/guide/08_error_handling.md`](../guide/08_error_handling.md) — `Lax` / `Result` / `Gorillax` の意味と `>=>` (アンモールド) の挙動
- [`docs/reference/diagnostic_codes.md`](../reference/diagnostic_codes.md) — `[E1602]` / `[E1960]` 等の診断コード一覧
