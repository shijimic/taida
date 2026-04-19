# `taida-lang/os` API リファレンス

> Core bundled package. `>>> taida-lang/os => @(...)` で import、または import 無しで直接呼び出し可能（両経路とも checker で同じ型 signature に pin されます）。

ガイドは [`docs/guide/14_os_package.md`](../guide/14_os_package.md) を参照。

---

## 1. ファイル I/O

### 入力（モールド）

| API | Signature | 備考 |
|-----|-----------|------|
| `Read[path]()` | `Str -> Lax[Str]` | UTF-8 テキスト、64MB 上限、BOM は除去しない |
| `readBytes(path)` | `Str -> Lax[Bytes]` | バイナリ、64MB 上限 |
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

### 2.2 Interactive 版（C19 以降）

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

## 4. 非同期入力（モールド）

| API | Signature |
|-----|-----------|
| `ReadAsync[path]()` | `Str -> Async[Lax[Str]]` |
| `HttpGet[url]()` | `Str -> Async[Lax[Str]]` |
| `HttpPost[url, body]()` | `(Str, Str) -> Async[Lax[Str]]` |
| `HttpRequest[method, url](headers, body)` | `(Str, Str, BuchiPack, Str) -> Async[Lax[Str]]` |

---

## 5. 非同期ソケット（関数）

| API | Signature |
|-----|-----------|
| `tcpConnect(host, port[, timeoutMs])` | `(Str, Int[, Int]) -> Async[Result[TcpSocket, IoError]]` |
| `tcpListen(port[, timeoutMs])` | `(Int[, Int]) -> Async[Result[TcpListener, IoError]]` |
| `tcpAccept(listener[, timeoutMs])` | `(TcpListener[, Int]) -> Async[Result[TcpSocket, IoError]]` |
| `socketSend(socket, data[, timeoutMs])` | `(TcpSocket, Str[, Int]) -> Async[Result[Int, IoError]]` |
| `socketSendAll(socket, data[, timeoutMs])` | `(TcpSocket, Str[, Int]) -> Async[Result[Unit, IoError]]` |
| `socketRecv(socket[, timeoutMs])` | `(TcpSocket[, Int]) -> Async[Result[Str, IoError]]` |
| `socketSendBytes(socket, data[, timeoutMs])` | `(TcpSocket, Bytes[, Int]) -> Async[Result[Int, IoError]]` |
| `socketRecvBytes(socket[, timeoutMs])` | `(TcpSocket[, Int]) -> Async[Result[Bytes, IoError]]` |
| `socketRecvExact(socket, size[, timeoutMs])` | `(TcpSocket, Int[, Int]) -> Async[Result[Bytes, IoError]]` |
| `udpBind(host, port[, timeoutMs])` | `(Str, Int[, Int]) -> Async[Result[UdpSocket, IoError]]` |
| `udpSendTo(socket, host, port, data[, timeoutMs])` | `(UdpSocket, Str, Int, Str[, Int]) -> Async[Result[Int, IoError]]` |
| `udpRecvFrom(socket[, timeoutMs])` | `(UdpSocket[, Int]) -> Async[Result[@(data: Str, host: Str, port: Int), IoError]]` |
| `socketClose(socket)` | `TcpSocket -> Result[Unit, IoError]` |
| `listenerClose(listener)` | `TcpListener -> Result[Unit, IoError]` |
| `udpClose(socket)` | `UdpSocket -> Result[Unit, IoError]` （`socketClose` のエイリアス） |

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

## 7. バージョン履歴

| バージョン | 変更 |
|------------|------|
| `@c.19.rc4` | `runInteractive` / `execShellInteractive` 追加。checker で `Gorillax[@(code: Int)]` を pin。Native の `__error` フィールドハッシュを修正 |
| `@c.18.rc4` 以前 | `run` / `execShell` （captured 版のみ）が存在 |
