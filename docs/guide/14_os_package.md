# OS パッケージ

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ

ファイルシステム・プロセス・環境変数・ネットワークといった OS 境界の操作は、`taida-lang/os` パッケージに集約されています。コアバンドル（install 不要）でプリリュード外ですが、import 文 1 行で全機能が使えます。

```taida
>>> taida-lang/os => @(Read, run, EnvVar)
```

import 無しの bare 呼び出しも許容されます（両者は checker 上で同じ型として pin されます）。

---

## 1. ファイル I/O

### 読み込み（モールド）

| API | 戻り値 | 用途 |
|-----|--------|------|
| `Read[path]()` | `Lax[Str]` | テキストファイル読み込み（UTF-8, 64MB 上限） |
| `readBytes(path)` | `Lax[Bytes]` | バイナリ読み込み（64MB 上限） |
| `ListDir[path]()` | `Lax[@[Str]]` | ディレクトリ列挙 |
| `Stat[path]()` | `Lax[@(size, modified, isDir)]` | メタデータ |
| `Exists[path]()` | `Bool` | 存在チェック |

```taida
>>> taida-lang/os => @(Read, Exists)

| !Exists["/etc/hosts"]() |> ><

content <= Read["/etc/hosts"]()
| content.hasValue |> stdout(content.__value)
| _                |> stderr(content.__error.message)
```

### 書き込み・変更（関数）

| API | 戻り値 | 用途 |
|-----|--------|------|
| `writeFile(path, content)` | `Result[Unit, IoError]` | 上書き / 新規作成 |
| `writeBytes(path, content)` | `Result[Unit, IoError]` | バイナリ書き込み |
| `appendFile(path, content)` | `Result[Unit, IoError]` | 追記 |
| `remove(path)` | `Result[Unit, IoError]` | 削除（ファイル / ディレクトリ） |
| `createDir(path)` | `Result[Unit, IoError]` | `mkdir -p` 相当 |
| `rename(from, to)` | `Result[Unit, IoError]` | 移動 / 改名（アトミック） |

---

## 2. プロセス起動

プロセス起動は「**出力を捕捉するか（captured）**」と「**端末を子に渡すか（interactive）**」で 4 種類あります。

| API | stdio | 戻り値 inner | 用途 |
|-----|-------|--------------|------|
| `run(program, args)` | pipe 捕捉 | `@(stdout: Str, stderr: Str, code: Int)` | 出力を読みたい CLI（curl, jq 等）|
| `execShell(command)` | pipe 捕捉 | `@(stdout: Str, stderr: Str, code: Int)` | shell 展開が必要なもの |
| `runInteractive(program, args)` | 親 TTY を継承 | `@(code: Int)` | TUI アプリ（nvim, less, fzf 等）|
| `execShellInteractive(command)` | 親 TTY を継承 | `@(code: Int)` | TUI + shell glob |

全て戻り値は `Gorillax[inner]` で、失敗時は `IoError`（fork/exec 失敗）か `ProcessError`（exit code 非ゼロ）を `__error` に詰めます。

### captured 版（run / execShell）

子プロセスの stdout / stderr を pipe で奪い、親から文字列として読めるようにします。外部コマンドの出力を解析したいときに使います。

```taida
>>> taida-lang/os => @(run)

r <= run("git", @["rev-parse", "HEAD"])
| !r.hasValue |>
  bytes <= stderr(r.__error.message)
  ><

sha <= Trim[r.__value.stdout]()
stdout("HEAD = " + sha)
```

### interactive 版（runInteractive / execShellInteractive, C19 以降）

子プロセスに親の stdin / stdout / stderr をそのまま継承します。端末を子が占有できるので、nvim のような TUI を起動できます。戻り値は exit code のみです。

```taida
>>> taida-lang/os => @(runInteractive)

// エディタを起動してユーザー編集を待つ
r <= runInteractive("nvim", @["/tmp/draft.md"])
| !r.hasValue |>
  bytes <= stderr("editor failed: " + r.__error.message)
  ><

stdout("editor exit: " + r.__value.code.toString())
```

`runInteractive` の戻り値には `stdout` / `stderr` フィールドが **存在しません**。checker が `Gorillax[@(code: Int)]` として pin しているため、`r.__value.stdout` を書くと compile error になります（型の誤用を実行前に弾く）。

```taida
r <= runInteractive("nvim", @["/tmp/draft.md"])
stdout(r.__value.stdout)   // ERROR: Field 'stdout' does not exist on type @(code: Int)
```

### 使い分けの指針

| 状況 | 推奨 |
|------|------|
| 外部コマンドの出力を文字列として読みたい | `run` / `execShell` |
| エディタ・ページャ・対話型 TUI を起動したい | `runInteractive` / `execShellInteractive` |
| shell 展開・pipe・リダイレクトが必須 | `execShell` 系（injection リスクに注意） |
| 固定引数で安全に起動したい | `run` 系（shell を挟まない） |

**Note**: `execShellInteractive` は `sh -c <command>` でラップするため、job control 層が 1 つ余計に挟まります。固定の単体プログラムで足りる場合は `runInteractive` を推奨します。

詳細な API リファレンスは [`docs/reference/os_api.md`](../reference/os_api.md) を参照してください。エディタ連携の典型パターンは [`docs/cookbook/editor_handoff.md`](../cookbook/editor_handoff.md) にまとまっています。

---

## 3. 環境変数・引数

| API | 戻り値 | 用途 |
|-----|--------|------|
| `EnvVar[name]()` | `Lax[Str]` | 単一環境変数（read-only）|
| `allEnv()` | `HashMap[Str, Str]` | 全環境変数 |
| `argv()` | `@[Str]` | CLI 引数（ユーザー引数のみ）|

```taida
>>> taida-lang/os => @(EnvVar, argv)

home <= EnvVar["HOME"]()
args <= argv()
stdout("home = " + home.getOrDefault("") + ", args = " + args.toString())
```

---

## 4. 非同期 I/O

ネットワーク・HTTP・非同期ファイル読み込みは `Async[T]` を返します。詳しくは [非同期処理](11_async.md) を参照してください。

```taida
>>> taida-lang/os => @(HttpGet)

resp <= HttpGet["https://example.com"]()
resp ]=> body
stdout(body.__value)
```

---

## 5. エラーハンドリング

全プロセス起動 API は `Gorillax` を返すので、`hasValue` / `__error` の二択で扱います。`IoError`（起動失敗）と `ProcessError`（exit code 非ゼロ）は `__error.kind` で区別できます。

```taida
>>> taida-lang/os => @(run)

r <= run("missing-program", @[])
| r.hasValue |> stdout("ok")
| r.__error.kind == "not_found" |> stderr("program not found")
| _                              |> stderr("exec failed: " + r.__error.message)
```

3 バックエンド（Interpreter / JS / Native）で `IoError.kind` / `IoError.code` の値は一致します（POSIX errno ベース）。
