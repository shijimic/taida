# 非同期処理

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ
>
> 系: 「待つ時は待つ、でも待ち方は考えない」

---

## Async[T] とは

`Async[T]` は「未来に値が得られる処理」を表現するモールド型です。I/O 操作やネットワーク通信だけでなく、明示的に遅延させた CPU 計算も同じ待機モデルで扱います。

`Async[T]` はビルトインのモールド型です。インポートは不要です。

---

## `>=>` による暗黙的 await

`>=>` でアンモールドすると、暗黙的に await として機能します。待ち方を考える必要はありません。

```taida
// 非同期操作の結果を取得します
result <= someAsyncOp()
result >=> data  // ここでブロックして解決を待ちます
stdout(data)
```

`async/await` のようなキーワードは存在しません。`>=>` が唯一の待機手段です。

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

複数の非同期操作を順番に実行するには、`>=>` で一つずつ結果を取り出します。

```taida
// ユーザーを取得して、そのユーザーの投稿を取得します
userAsync <= fetchUser(1)
userAsync >=> user
postsAsync <= fetchPosts(user.id)
postsAsync >=> posts
stdout(posts)
```

関数にまとめることもできます。

```taida
fetchUserPosts userId: Int =
  |== error: Error =
    @[]
  => :@[Post]

  userAsync <= fetchUser(userId)
  userAsync >=> user
  postsAsync <= fetchPosts(user.id)
  postsAsync >=> posts
  posts
=> :@[Post]
```

---

## map による同期的変換

`map` メソッドで、非同期値を同期的に変換できます。非同期操作自体は増えません。

```taida
nameAsync <= fetchUser(1).map(_ user = Upper[user.name]())
// nameAsync: Async[Str]

nameAsync >=> name
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

エラー天井 `|==` を使って非同期操作のエラーを処理します。`>=>` でアンモールドした際にエラーが発生すると、エラー天井にジャンプします。

```taida
fetchData id: Int =
  |== error: Error =
    @(found <= false, data <= "")
  => :@(found: Bool, data: Str)

  result <= someAsyncOp(id)
  result >=> data  // 拒否された場合、ここでエラー天井へジャンプします

  @(found <= true, data <= data)
=> :@(found: Bool, data: Str)
```

エラー天井 `|==` がない場合は、ゴリラ天井がキャッチしてプログラムを停止させます。

### 旧 JS ターゲットの Promise 境界

旧 JS ターゲットで Promise-returning 関数を呼ぶ場合は、
`Cage[subject, JSCallAsync[path, args, Out]()]()` を使います。この形は
`Async[Out]` を返します。Promise rejection は `Async` の rejection に
変換され、`>=>` で待った位置の `|==` エラー天井で捕捉できます。

```taida
>>> npm:node:timers/promises => @(setTimeout)

waitAnswer =
  task <= Cage[setTimeout, JSCallAsync[@[], @[20, 42], Int]()]()
  task >=> value
  value
=> :Int
```

同期 JS 呼び出しの `JSCall` は `Gorillax[Out]` 境界です。Promise を返す
関数に対して `JSCall[..., Async[Out]]` とは書きません。

### リトライパターン

末尾再帰と組み合わせたリトライパターンです。

```taida
retryLoop maxRetries: Int attempt: Int =
  |== error: Error =
    | attempt < maxRetries |> retryLoop(maxRetries, attempt + 1)
    | _ |> @(success <= false, data <= "")
  => :@(success: Bool, data: Str)

  result <= riskyAsyncOp()
  result >=> data
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
allResult >=> results  // @[1, 2, 3]

// Race: 最初に完了した結果を返します
raceResult <= Race[asyncOps]()
raceResult >=> winner  // 最初の要素が返されます

// Timeout: タイムアウト付きで結果を待ちます
firstAsync <= asyncOps.first().getOrDefault(Async[0]())
timeoutResult <= Timeout[firstAsync, 5000]()
timeoutResult >=> data  // タイムアウト前に完了すれば値を返します

// nowMs/sleep: 最小時間プリミティブ
start <= nowMs()
wait <= sleep(20)
wait >=> _done
end <= nowMs()
stdout((end - start).toString())
```

`sleep(ms)` は `Async[Int]` を返します。解決値は実際に経過した ms 数 (`ms` と同値が基本) です。`ms` は `Int` かつ `0..=2_147_483_647` の範囲です。範囲外は rejected `Async` になります。

`nowMs()` は wall-clock（epoch ミリ秒）であり、単調時計ではありません。厳密な経過時間測定には差分と許容誤差を併用してください。

---

## CPU タスクの明示並列

CPU 計算を未来の値として扱う場合は、`AsyncTask` で遅延タスクを作り、`Par` または `ParMap` でまとめて `Async` にします。`Par` / `ParMap` の結果リストは、実行完了順ではなく入力順で安定します。

```taida
square x: Int =
  x * x
=> :Int

jobs <= @[
  AsyncTask[_ = square(3)](),
  AsyncTask[_ = square(4)](),
  AsyncTask[_ = square(5)](),
]

Par[jobs]() >=> values
stdout(values.toString())  // @[9, 16, 25]
```

`ParMap` はリストの各要素へ同じ関数を適用し、結果を入力順のリストとして返します。

```taida
items <= @[1, 2, 3]
ParMap[items, _ x: Int = x * 2]() >=> doubled
stdout(doubled.toString())  // @[2, 4, 6]
```

`AsyncTask` の本体と `ParMap` の mapper は、worker 境界を越えられる純粋な計算に限定されます。I/O、ネットワーク、プロセス操作、host interop、ネストした `AsyncTask` / `Par` / `ParMap`、関数値や `Async` / `Stream` / `Molten` などの捕捉は、実行時失敗ではなく checker 診断で reject されます。

addon 関数は既定では worker 内で拒否されます。例外は、`native/addon.toml` で関数単位に `"declared"` purity claim があり、利用側 `packages.tdm` の `[parallelism] addon_purity` がその claim を許可している場合の直接呼び出しだけです。addon 関数を変数へ入れて worker に捕捉する形は、purity claim に関係なく拒否されます。

```toml
[parallelism]
addon_purity = "allow declared"
```

バックエンドは worker dispatch または deterministic sequential fallback のどちらで実行してもよく、公開される契約は同じです。結果順、失敗伝播、`>=>` の待機挙動はバックエンド間で一致します。

---

## クロージャの値キャプチャ

非同期ラムダ（`Async[T]` を返すクロージャ）が外部スコープの値を参照する場合、値はラムダ生成時点でその場の値として取り込まれます。ラムダ本体は `>=>` の中断ポイントをまたいでもキャプチャ済みの値を使い続けます。生成後に別の値を作っても、ラムダの中の値は影響を受けません。

```taida
counter <= 0
build value: Int =
  Async[value + 1]()
=> :Async[Int]

job <= build(counter)   // value = 0 がキャプチャされる
counter_after <= 99     // 生成後の別値は job に影響しない
job >=> result          // result == 1
```

この「値キャプチャ」挙動と `Async[T]` の構文・型形状は、安定リリースの仕様として固定されています。参照キャプチャや遅延束縛のような別の意味論を導入する場合は、世代をまたぐ変更として扱います。

---

## Blocking Addon Caveat

`taida-lang/terminal` の `readEvent()` のように OS の blocking I/O を内部で使う addon は、Taida 側の `Async[T]` から呼ぶ場合でも、同じ入力ストリームにつき dedicated blocking thread から呼び出してください。`readEvent()` は未消費 byte queue を thread-local framing context として保持するため、複数 OS thread から同じ stdin を並行に読む設計では thread ごとに独立した queue になります。

Tokio などの multi-thread runtime では、`spawn_blocking` 相当の単一専用 worker に `readEvent()` を寄せるのが契約です。これは public signature を変えずに FIFO を保証するための host-side 制約です。

---

## まとめ

| 概念 | 構文 |
|------|------|
| Async の await | `async_value >=> result` |
| エラーハンドリング | `\|== error: Error = ...` |
| 同期的変換 | `.map(_ x = ...)` |
| 非ブロッキング取得 | `.getOrDefault(value)` |
| 即時解決 | `Async[value]()` |
| 即時拒否 | `AsyncReject[error]()` |
| CPU タスク | `AsyncTask[_ = expr]()` |
| CPU タスクの合成 | `Par[jobs]()` / `ParMap[list, fn]()` |

前のガイド: [モジュールシステム](10_modules.md) | 次のガイド: [構造的イントロスペクション](12_introspection.md)
