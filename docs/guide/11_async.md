# 非同期処理

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ
>
> 系: 「待つ時は待つ、でも待ち方は考えない」

---

## Async[T] とは

`Async[T]` は非同期操作を表現するモールディング型です。I/O 操作やネットワーク通信など、完了までに時間がかかる処理を型安全に扱います。

`Async[T]` はビルトインのモールディング型です。インポートは不要です。

---

## `]=>` による暗黙的 await

`]=>` でアンモールディングすると、暗黙的に await として機能します。待ち方を考える必要はありません。

```taida
// 非同期操作の結果を取得します
result <= someAsyncOp()
result ]=> data  // ここでブロックして解決を待ちます
stdout(data)
```

`async/await` のようなキーワードは存在しません。`]=>` が唯一の待機手段です。

---

## 即時解決 / 即時拒否

テストやモック用に、即座に解決・拒否する Async を作成できます。

```taida
// 即座に解決します
resolved <= Async[42]()

// 即座に拒否します（AsyncReject モールドを使用）
rejected <= AsyncReject["timeout error"]()
```

---

## 非同期操作の連鎖

複数の非同期操作を順番に実行するには、`]=>` で一つずつ結果を取り出します。

```taida
// ユーザーを取得して、そのユーザーの投稿を取得します
userAsync <= fetchUser(1)
userAsync ]=> user
postsAsync <= fetchPosts(user.id)
postsAsync ]=> posts
stdout(posts)
```

関数にまとめることもできます。

```taida
fetchUserPosts userId: Int =
  |== error: Error =
    @[]
  => :@[Post]

  userAsync <= fetchUser(userId)
  userAsync ]=> user
  postsAsync <= fetchPosts(user.id)
  postsAsync ]=> posts
  posts
=> :@[Post]
```

---

## map による同期的変換

`map` メソッドで、非同期値を同期的に変換できます。非同期操作自体は増えません。

```taida
nameAsync <= fetchUser(1)
  .map(_ user = Upper[user.name]())
// nameAsync: Async[Str]

nameAsync ]=> name
stdout(name)  // 大文字に変換されたユーザー名
```

---

## 非ブロッキング取得

`getOrDefault` を使うと、解決を待たずにデフォルト値を返すことができます。

```taida
result <= someAsyncOp()
data <= result.getOrDefault(@(items <= @[]))
// pending や rejected の場合はデフォルト値を返します
```

---

## エラーハンドリング

エラー天井 `|==` を使って非同期操作のエラーを処理します。`]=>` でアンモールディングした際にエラーが発生すると、エラー天井にジャンプします。

```taida
fetchData id: Int =
  |== error: Error =
    @(found <= false, data <= "")
  => :@(found: Bool, data: Str)

  result <= someAsyncOp(id)
  result ]=> data  // 拒否された場合、ここでエラー天井へジャンプします

  @(found <= true, data <= data)
=> :@(found: Bool, data: Str)
```

エラー天井 `|==` がない場合は、ゴリラ天井がキャッチしてプログラムを停止させます。

### リトライパターン

末尾再帰と組み合わせたリトライパターンです。

```taida
retryLoop maxRetries: Int attempt: Int =
  |== error: Error =
    | attempt < maxRetries |> retryLoop(maxRetries, attempt + 1)
    | _ |> @(success <= false, data <= "")
  => :@(success: Bool, data: Str)

  result <= riskyAsyncOp()
  result ]=> data
  @(success <= true, data <= data)
=> :@(success: Bool, data: Str)

// 最大3回リトライ
outcome <= retryLoop(3, 0)
```

---

## Async[T] メソッド一覧

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `isPending()` | `=> :Bool` | 未解決かどうかを返します |
| `isFulfilled()` | `=> :Bool` | 解決済みかどうかを返します |
| `isRejected()` | `=> :Bool` | 拒否されたかどうかを返します |
| `map(fn)` | `:T => :U => :Async[U]` | 値の同期変換を行います |
| `getOrDefault(default)` | `T => :T` | デフォルト値付きの非ブロッキング取得です |
| `unmold()` | `=> :T` | 値を取り出します（ブロッキング await） |
| `toString()` | `=> :Str` | 文字列表現を返します |

---

## 並列実行とタイムアウト

`All`、`Race`、`Timeout` の各モールドに加えて、時間系 Prelude 最小カーネル `nowMs` / `sleep` が利用できます。

```taida
// All: 全ての非同期操作の完了を待ちます
asyncOps <= @[Async[1](), Async[2](), Async[3]()]
allResult <= All[asyncOps]()
allResult ]=> results  // @[1, 2, 3]

// Race: 最初に完了した結果を返します
raceResult <= Race[asyncOps]()
raceResult ]=> winner  // 最初の要素が返されます

// Timeout: タイムアウト付きで結果を待ちます
timeoutResult <= Timeout[someAsync, 5000]()
timeoutResult ]=> data  // タイムアウト前に完了すれば値を返します

// nowMs/sleep: 最小時間プリミティブ
start <= nowMs()
wait <= sleep(20)
wait ]=> _done
end <= nowMs()
stdout((end - start).toString())
```

`sleep(ms)` は `Async[Unit]` を返します。`ms` は `Int` かつ `0..=2_147_483_647` の範囲です。範囲外は rejected `Async` になります。

`nowMs()` は wall-clock（epoch ミリ秒）であり、単調時計ではありません。厳密な経過時間測定には差分と許容誤差を併用してください。

---

## Blocking Addon Caveat

`terminal.ReadEvent[]()` のように OS の blocking I/O を内部で使う addon は、Taida 側の `Async[T]` から呼ぶ場合でも、同じ入力ストリームにつき dedicated blocking thread から呼び出してください。C26B-012 以降の `taida-lang/terminal` は `PENDING_BYTES` を thread-local framing context として保持するため、複数 OS thread から同じ stdin を並行に読む設計では thread ごとに独立した未消費 byte queue になります。

Tokio などの multi-thread runtime では、`spawn_blocking` 相当の単一専用 worker に `ReadEvent[]()` を寄せるのが契約です。これは public signature を変えずに FIFO を保証するための host-side 制約です。

---

## まとめ

| 概念 | 構文 |
|------|------|
| Async の await | `asyncValue ]=> result` |
| エラーハンドリング | `\|== error: Error = ...` |
| 同期的変換 | `.map(_ x = ...)` |
| 非ブロッキング取得 | `.getOrDefault(value)` |
| 即時解決 | `Async[value]()` |
| 即時拒否 | `AsyncReject[error]()` |

前のガイド: [モジュールシステム](10_modules.md) | 次のガイド: [構造的イントロスペクション](12_introspection.md)
