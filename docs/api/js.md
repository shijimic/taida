# `taida-lang/js` API リファレンス

`taida-lang/js` は旧 JS ターゲット向けの互換 API です。JavaScript
エコシステムとの相互運用 (interop) を `Cage` モールドと組み合わせて
表現するため、外部 JavaScript ライブラリのオブジェクトやメソッドを
呼び出す「branch capability 実行記述」群を公開します。

この API は正式なリリースパリティ対象ではありません。新規の portable
コードは Interpreter / Native / WASM を前提に設計し、JavaScript 固有の
境界が必要な既存コードの移行期間に限って使ってください。

```taida
>>> taida-lang/js => @(JSGet, JSCall, JSCallAsync, JSNew, JSSet, JSBind, JSSpread)
```

これらはすべて `Cage[subject, runner]()` の `runner` 位置に置く
実行記述 (`JSRilla` 系) として使われます。直接呼ぶ API ではなく、
`Cage` の中で評価されることを前提とします。

`Cage` 自体の仕様は
[`docs/api/prelude.md §7.3`](prelude.md#73-モールド型コンストラクタ)
を、`Gorillax` / `RelaxedGorillax` のメソッドは
[`docs/api/prelude.md §8.7`](prelude.md#87-gorillax--relaxedgorillax-メソッド)
を参照してください。

---

## 1. プロパティ取得

### 1.1 `JSGet`

```taida fragment
JSGet[path: @[Str], Out]() => :JSRilla[Out]
```

JS オブジェクトのプロパティを取得する 実行記述 を作ります。`path` で
表されるドット区切り経路を順に辿り、最終値を `Out` 型として取り出し
ます。

- `path` は `@["a", "b", "c"]` のように **文字列リスト** で表現します
  (`obj.a.b.c` 相当)。
- `Out` は取り出し後の Taida 型 (`Str` / `Int` / `Float` / `Bool` /
  `Molten` 等)。`@()` / `:Unit` / `:Void` 型は禁止 (`[E1520]`)。
- `path` が存在しない場合、`Cage` 越境失敗として `Gorillax` の失敗側に
  落ちます。

```taida fragment
Cage[lodash, JSGet[@["VERSION"], Str]()]() >=> version
```

---

## 2. 関数呼び出し

### 2.1 `JSCall`

```taida fragment
JSCall[path: @[Str], args: @[Any], Out]() => :JSRilla[Out]
```

JS の関数 / メソッドを呼び出す 実行記述 を作ります。`path` で指定
した関数を `args` を引数として呼び出し、戻り値を `Out` 型として取り
出します。

- `path` は呼び出し対象関数までの経路 (`@["sum"]` で global `sum`、
  `@["math", "max"]` で `math.max`)。
- `args` は JS に渡す Taida 値のリスト。`Str` / `Int` / `Float` /
  `Bool` / `Molten` を含められます。
- `JSCall` は同期境界です。`Cage[subject, JSCall[...]]()` は
  `Gorillax[Out]` を返します。
- 戻り値が JS の Promise の関数は `JSCallAsync` を使います。
  `JSCall[..., Async[U]]` とは書きません (`[E1519]`)。
- `Out` に `@()` / `:Unit` / `:Void` を指定することはできません
  (`[E1520]`)。代入のような副作用呼び出しは `JSSet`、戻り値を持つ
  メソッドは具体型 (例: `Array.prototype.push` は `Int`) を指定します。

```taida fragment
Cage[underscore, JSCall[@["sum"], @[items], Int]()]() >=> total
```

### 2.2 `JSCallAsync`

```taida fragment
JSCallAsync[path: @[Str], args: @[Any], Out]() => :JSRilla[Out]
```

JS の Promise-returning 関数 / メソッドを呼び出す 実行記述 を作ります。
`path` と `args` の規則は `JSCall` と同じですが、`Cage` の戻り値は
`Gorillax[Out]` ではなく `Async[Out]` です。Promise rejection は
`Async` の rejection になり、`>=>` で待った位置の `|==` エラー天井で
捕捉できます。

`Out` は Promise が解決した後の型です。`JSCallAsync[..., Async[U]]`
とは書きません (`[E1519]`)。対象関数が Promise ではない値を返した
場合は JS boundary error として rejected `Async` になります。

```taida
>>> npm:node:timers/promises => @(setTimeout)

Cage[setTimeout, JSCallAsync[@[], @[20, 42], Int]()]() >=> value
```

### 2.3 `JSNew`

```taida fragment
JSNew[path: @[Str], args: @[Any], Out]() => :JSRilla[Out]
```

JS のコンストラクタを `new` 呼び出しする 実行記述 を作ります。
それ以外は `JSCall` と同じ契約です。

```taida fragment
Cage[Date, JSNew[@[], @["2026-01-01"], Molten]()]() >=> date
```

---

## 3. プロパティ代入

### 3.1 `JSSet`

```taida fragment
JSSet[path: @[Str], value: Any]() => :JSRilla[Bool]
```

JS オブジェクトのプロパティに値を代入する 実行記述 を作ります。
代入操作は副作用を持ち、戻り値は代入成否を示す `Bool` です。
JS の代入は throw が起きない限り常に成功するため、通常は `true` が
返ります。

- `path` は代入対象プロパティまでの経路。
- `value` は Taida 側の値。

```taida fragment
Cage[config, JSSet[@["debug"], true]()]() >=> ok
```

Taida 側の値は不変ですが、`Cage` を介して呼び出した JS オブジェクトに
対しては副作用としての代入が許容されます。代入後の JS オブジェクトを
Taida 側で参照する場合は別途 `JSGet` で取り出してください。

---

## 4. バインド / スプレッド

### 4.1 `JSBind`

```taida fragment
JSBind[path: @[Str]]() => :JSRilla[Molten]
```

JS のメソッドを `this` に束縛した状態で取り出す 実行記述 を作ります。
JS の `obj.method.bind(obj)` 相当です。戻り値は `Molten` (不透明
ハンドル) で、後続の `JSCall` に渡せます。

```taida fragment
Cage[arr, JSBind[@["push"]]()]() >=> pushFn
Cage[arr, JSCall[@["push"], @[item], Int]()]() >=> newLength   // 等価。push は新しい配列長を返す
```

### 4.2 `JSSpread`

```taida fragment
JSSpread[source: @[Str]]() => :JSRilla[Molten]
```

JS のスプレッド演算子 (`...source`) を呼び出し位置の引数列に展開する
実行記述 を作ります。`JSCall` の `args` 内で他の値と組み合わせて
利用します。

```taida fragment
Cage[Math, JSCall[@["max"], @[JSSpread[@["values"]]()], Int]()]() >=> peak
```

---

## 5. バックエンド対応

| バックエンド | 対応範囲 |
|--------------|----------|
| インタプリタ | 利用不可 (JavaScript ホストが必要) |
| ネイティブ | 利用不可 |
| 旧 JS ターゲット | 全シンボル対応 |
| WASM (全プロファイル) | 利用不可 |

`taida-lang/js` は **旧 JS ターゲット専用** です。インタプリタや
ネイティブ / WASM バックエンドからこれらのシンボルを呼ぶと、決定的な
コンパイル時エラーが返ります。クロスプラットフォームで動作するコード
を書く場合は、`taida-lang/js` に依存しない設計にしてください。

---

## 関連リファレンス

- [`README.md`](README.md) — `docs/api/` 全体の入口
- [`docs/api/prelude.md`](prelude.md) — `Cage` / `Gorillax` / `RelaxedGorillax` のメソッドとプレリュード関数
- [`docs/guide/08_error_handling.md`](../guide/08_error_handling.md) — `Gorillax` / `RelaxedGorillax` / `errorInfo` の扱い
