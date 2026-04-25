# `taida-lang/os` API リファレンス

> Core bundled package. `>>> taida-lang/os => @(...)` で import、または import 無しで直接呼び出し可能（両経路とも checker で同じ型 signature に pin されます）。

ガイドは [`docs/guide/14_os_package.md`](../guide/14_os_package.md) を参照。

---

## 1. ファイル I/O

### 入力（モールド）

| API | Signature | 備考 |
|-----|-----------|------|
| `Read[path]()` | `Str -> Lax[Str]` | UTF-8 テキスト、64MB 上限、BOM は除去しない |
| `readBytes(path)` | `Str -> Lax[Bytes]` | バイナリ全読、64MB 上限 |
| `readBytesAt(path, offset, len)` | `(Str, Int, Int) -> Lax[Bytes]` | チャンク読み込み。`offset` は 0-indexed、1 回呼び出しの `len` 上限は 64MB。EOF 越えは可読部分のみ返す。`offset` / `len` 不正は `Lax` の default |
| `ListDir[path]()` | `Str -> Lax[@[Str]]` | ファイル名のみ（パス無し） |
| `Stat[path]()` | `Str -> Lax[@(size: Int, modified: Int, isDir: Bool)]` | `modified` は epoch ミリ秒 |
| `Exists[path]()` | `Str -> Bool` | symlink は follow する |

### 出力（関数）

| API | Signature | 備考 |
|-----|-----------|------|
| `writeFile(path, content)` | `(Str, Str) -> Result[Unit, IoError]` | 上書き。ディレクトリは自動作成しない |
| `writeBytes(path, content)` | `(Str, Bytes) -> Result[Unit, IoError]` | 上書き |
| `appendFile(path, content)` | `(Str, Str) -> Result[Unit, IoError]` | 追記。無ければ新規作成 |
| `remove(path)` | `Str -> Result[Unit, IoError]` | ファイル / 空ディレクトリ |
| `createDir(path)` | `Str -> Result[Unit, IoError]` | `mkdir -p` 相当 |
| `rename(from, to)` | `(Str, Str) -> Result[Unit, IoError]` | アトミック（同一 filesystem 内） |

---

## 2. プロセス起動

### 2.1 Captured 版

stdout / stderr を pipe で捕捉し、文字列として返します。

| API | Signature |
|-----|-----------|
| `run(program, args)` | `(Str, @[Str]) -> Gorillax[@(stdout: Str, stderr: Str, code: Int)]` |
| `execShell(command)` | `Str -> Gorillax[@(stdout: Str, stderr: Str, code: Int)]` |

- `run` は `execvp` 直呼び（shell を挟まない）。引数はそのまま argv[1..] に渡る
- `execShell` は `sh -c <command>` （POSIX）/ `cmd /C <command>` （Windows）でラップ。shell 展開・pipe・リダイレクトが使える一方、shell injection リスクあり

**Failure modes**:
- fork/exec 失敗 → `__error` は `IoError` （`kind`, `code` = POSIX errno）
- 非ゼロ exit → `__error` は `ProcessError` （`code`, `stdout`, `stderr`）

### 2.2 Interactive 版

親の stdin / stdout / stderr を子に継承させます。戻り値は exit code のみ。

| API | Signature |
|-----|-----------|
| `runInteractive(program, args)` | `(Str, @[Str]) -> Gorillax[@(code: Int)]` |
| `execShellInteractive(command)` | `Str -> Gorillax[@(code: Int)]` |

- stdout / stderr は **捕捉しない**。子プロセスが親の端末を直接占有する
- `__value` の inner shape は **`@(code: Int)` のみ**。`.stdout` / `.stderr` へのアクセスは checker で E1602 として reject される
- 3 バックエンドの実装:
  - Interpreter: `std::process::Command::status()`
  - JS: `child_process.spawnSync(prog, args, { stdio: 'inherit' })`
  - Native: `fork()` → `execvp()`（dup2 なし）→ `waitpid()`

**Failure modes**:
- fork/exec 失敗 → `__error` は `IoError` （`kind`, `code` = POSIX errno、Native は CLOEXEC errno pipe で伝搬）
- 非ゼロ exit → `__error` は `ProcessError` （`code` のみ）
- Signal death → `code = 128 + signal` 規約（POSIX）。Windows は best-effort

### 2.3 使い分け早見表

| 目的 | API |
|------|-----|
| 出力を解析する（curl, jq, git log 等） | `run` / `execShell` |
| TUI を起動する（nvim, less, fzf 等） | `runInteractive` / `execShellInteractive` |
| shell 展開が必要 | `execShell` 系 |
| 固定引数で安全起動 | `run` 系 |

---

## 3. 環境変数・引数

| API | Signature | 備考 |
|-----|-----------|------|
| `EnvVar[name]()` | `Str -> Lax[Str]` | 未設定時は `Lax` の default |
| `allEnv()` | `() -> HashMap[Str, Str]` | snapshot |
| `argv()` | `() -> @[Str]` | ユーザー引数（`taida run script.td -- a b` の `a b` 部分） |

---

## 3.5 標準入力（prelude）

| API | Signature | 備考 |
|-----|-----------|------|
| `stdin(prompt?)` | `() -> Str` / `Str -> Str` | cooked-mode 1 行読み取り。EOF / IO エラー時は `""`（失敗検知不可）。ASCII 入力 / pipe 用途向け |
| `stdinLine(prompt?)` | `() -> Async[Lax[Str]]` / `Str -> Async[Lax[Str]]` | UTF-8-aware line editor (rustyline / readline/promises / linenoise 派生)。caller は `]=>` で unmold して `Lax[Str]` を得る。EOF / Ctrl-C / Ctrl-D で `Lax.failure("")` |

`stdinLine` の典型的な使い方:

```taida
stdinLine("お名前: ") ]=> line
stdout("こんにちは、" + line.getOrDefault("旅人"))
```

`stdin` vs `stdinLine` の使い分けはガイド [14_os_package.md §1.5a](../guide/14_os_package.md) を参照。対話 CLI / multibyte 入力は `stdinLine` を推奨します。

どちらも import なしの prelude 関数です（`>>> taida-lang/os` は不要）。

---

## 4. 非同期入力（モールド）

| API | Signature |
|-----|-----------|
| `ReadAsync[path]()` | `Str -> Async[Lax[Str]]` |
| `HttpGet[url]()` | `Str -> Async[Lax[Str]]` |
| `HttpPost[url, body]()` | `(Str, Str) -> Async[Lax[Str]]` |
| `HttpRequest[method, url](headers, body)` | `(Str, Str, BuchiPack \| List[@(name: Str, value: Str)], Str) -> Async[Lax[@(status: Int, body: Str, headers: BuchiPack)]]` |

`HttpRequest` の `headers` 引数は 2 形式を受け付けます（どちらも 3 バックエンドで等価）:

- buchi-pack 形式: `headers <= @(content_type <= "application/json")` — フィールド識別子がそのまま wire header 名になります（`-` や `.` は識別子に使えないので `x-api-key` 等は書けません）。
- 名前-値ペアリスト形式: `headers <= @[@(name <= "x-api-key", value <= "secret"), @(name <= "anthropic-version", value <= "2023-06-01")]` — `List[@(name: Str, value: Str)]`。任意 UTF-8 header 名が使えます。

`-` を含む header 名 (`x-api-key` 等) を扱う場合はペアリスト形式を使用してください。`HttpRequest["GET"]()` のように type arg が 2 未満の呼び出しは Interpreter / JS / Native すべてで拒否されます（JS backend は `taida build --target js` 時点で arity error）。

---

## 5. 非同期ソケット（関数）

| API | Signature | 備考 |
|-----|-----------|------|
| `tcpConnect(host, port[, timeoutMs])` | `(Str, Int[, Int]) -> Async[Result[TcpSocket, IoError]]` | TCP client 接続 |
| `tcpListen(port[, timeoutMs])` | `(Int[, Int]) -> Async[Result[TcpListener, IoError]]` | TCP server bind + listen |
| `tcpAccept(listener[, timeoutMs])` | `(TcpListener[, Int]) -> Async[Result[TcpSocket, IoError]]` | accept 1 connection |
| `socketSend(socket, data[, timeoutMs])` | `(TcpSocket, Str[, Int]) -> Async[Result[Int, IoError]]` | partial send 可、戻り値は送信 byte 数 |
| `socketSendAll(socket, data[, timeoutMs])` | `(TcpSocket, Str[, Int]) -> Async[Result[Unit, IoError]]` | 全 byte 送信完了まで block |
| `socketRecv(socket[, timeoutMs])` | `(TcpSocket[, Int]) -> Async[Result[Str, IoError]]` | best-effort 受信 |
| `socketSendBytes(socket, data[, timeoutMs])` | `(TcpSocket, Bytes[, Int]) -> Async[Result[Int, IoError]]` | binary 送信 |
| `socketRecvBytes(socket[, timeoutMs])` | `(TcpSocket[, Int]) -> Async[Result[Bytes, IoError]]` | binary 受信 |
| `socketRecvExact(socket, size[, timeoutMs])` | `(TcpSocket, Int[, Int]) -> Async[Result[Bytes, IoError]]` | `size` byte 揃うまで待機 |
| `udpBind(host, port[, timeoutMs])` | `(Str, Int[, Int]) -> Async[Result[UdpSocket, IoError]]` | UDP socket bind |
| `udpSendTo(socket, host, port, data[, timeoutMs])` | `(UdpSocket, Str, Int, Str[, Int]) -> Async[Result[Int, IoError]]` | UDP datagram 送信 |
| `udpRecvFrom(socket[, timeoutMs])` | `(UdpSocket[, Int]) -> Async[Result[@(data: Str, host: Str, port: Int), IoError]]` | UDP datagram 受信 |
| `socketClose(socket)` | `TcpSocket -> Result[Unit, IoError]` | TCP socket（`tcpConnect` / `tcpAccept` の戻り値）を閉じる |
| `listenerClose(listener)` | `TcpListener -> Result[Unit, IoError]` | TCP listener（`tcpListen` の戻り値）を閉じる。`tcpAccept` で受理済 socket は別途 `socketClose` 必須 |
| `udpClose(socket)` | `UdpSocket -> Result[Unit, IoError]` | UDP socket（`udpBind` の戻り値）を閉じる。socket type が異なるため `socketClose` と呼び分け必須（型不整合は `[E1602]` で reject） |

---

## 6. エラー型

### `IoError` （起動・I/O 失敗時）

```
@(
  message: Str,
  kind: Str,    // "not_found", "permission_denied", "already_exists", ...
  code: Int     // POSIX errno (ENOENT=2, EACCES=13, ...)
)
```

### `ProcessError` （exit code 非ゼロ時）

Captured 版:
```
@(code: Int, stdout: Str, stderr: Str)
```

Interactive 版:
```
@(code: Int)
```

---

## 7. パス境界ポリシー（import / module loader）

`>>> ./X.td` や `>>> ./sub/Y.td` といった相対 import は
**プロジェクトルート（`.tdproj` のあるディレクトリ）** を境界とします。
3 backend (Interpreter / JS / Native) で同一の境界判定とエラー
メッセージを返します。

| パターン | 例 | 結果 |
|---------|-----|------|
| プロジェクトルート内に閉じた `..` | `>>> ./sub/../sibling.td` | **許容**（解決後パスが root 内にあるため） |
| プロジェクトルート内に閉じた nested `..` | `>>> ./sub/nested/../../sibling.td` | **許容** |
| プロジェクトルート外への relative traversal | `>>> ./../outside.td` | **reject** |
| プロジェクトルート外への absolute path | `>>> /tmp/outside/file.td` | **reject**（3 backend symmetric） |

reject 時は 3 backend で **同一の error 文字列** を返します:

```
Import path '<exact import token>' resolves outside the project root. Path traversal beyond the project boundary is not allowed.
```

`<exact import token>` には import 元コードに書かれた文字列がそのまま
入ります（`./../outside.td` であればそのリテラル、`/tmp/outside/file.td`
であればそのリテラル）。lexical-vs-resolved の区別はなく、wire 上は
入力 token を保ったまま fail-fast します。

実装参照:

- Interpreter: `src/interpreter/module_eval.rs`
- JS codegen: `src/js/codegen.rs`
- Native codegen: `src/codegen/driver.rs`
- 3 backend parity guard は `tests/` 配下の path traversal parity fixtures（5 cases × 3 backends）

> **Note**: pkg-store URL component validation
> (`src/pkg/store.rs::validate_path_component`) は本セクションの import path
> 境界判定とは domain disjoint です（重複なし）。
