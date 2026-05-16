# `taida-lang/js` API リファレンス

`taida-lang/js` は JavaScript エコシステムとの相互運用 (interop) を
提供するコア同梱パッケージです。`Cage` モールドと組み合わせて、外部
JavaScript ライブラリのオブジェクトやメソッドを Taida 側から安全に
呼び出すための「branch capability descriptor」群を公開します。

```taida
>>> taida-lang/js => @(JSGet, JSCall, JSNew, JSSet, JSBind, JSSpread)
```

これらはすべて `Cage[subject, runner]()` の `runner` 位置に置く
descriptor (`CageRilla` ファミリーの `JSRilla` 系) として使われます。
直接呼ぶ API ではなく、`Cage` の中で評価されることを前提とします。

`Cage` 自体の仕様は
[`docs/api/prelude.md §7.3`](prelude.md#73-モールド型コンストラクタ)
を、`Gorillax` / `RelaxedGorillax` のメソッドは
[`docs/api/prelude.md §8.7`](prelude.md#87-gorillax--relaxedgorillax-メソッド)
を参照してください。

---

## 1. プロパティ取得

### 1.1 `JSGet`

```taida
JSGet[path: @[Str], T]() => :CageRilla[T]
```

JS オブジェクトのプロパティを取得する descriptor を作ります。`path` で
表されるドット区切り経路を順に辿り、最終値を `T` 型として取り出し
ます。

- `path` は `@["a", "b", "c"]` のように **文字列リスト** で表現します
  (`obj.a.b.c` 相当)。
- `T` は取り出し後の Taida 型 (`Str` / `Int` / `Float` / `Bool` /
  `Molten` 等)。
- `path` が存在しない場合、`Cage` 越境失敗として `Gorillax` の失敗側に
  落ちます。

```taida
Cage[lodash, JSGet[@["VERSION"], Str]()]() ]=> version
```

---

## 2. 関数呼び出し

### 2.1 `JSCall`

```taida
JSCall[path: @[Str], args: @[Any], T]() => :CageRilla[T]
```

JS の関数 / メソッドを呼び出す descriptor を作ります。`path` で指定
した関数を `args` を引数として呼び出し、戻り値を `T` 型として取り
出します。

- `path` は呼び出し対象関数までの経路 (`@["sum"]` で global `sum`、
  `@["math", "max"]` で `math.max`)。
- `args` は JS に渡す Taida 値のリスト。`Str` / `Int` / `Float` /
  `Bool` / `Molten` を含められます。
- 戻り値が JS の Promise の場合、`T` を `Async[U]` として宣言すれば
  Taida 側で `]=>` で待てます。

```taida
Cage[underscore, JSCall[@["sum"], @[items], Int]()]() ]=> total
```

### 2.2 `JSNew`

```taida
JSNew[path: @[Str], args: @[Any], T]() => :CageRilla[T]
```

JS のコンストラクタを `new` 呼び出しする descriptor を作ります。
それ以外は `JSCall` と同じ契約です。

```taida
Cage[Date, JSNew[@[], @["2026-01-01"], Molten]()]() ]=> date
```

---

## 3. プロパティ代入

### 3.1 `JSSet`

```taida
JSSet[path: @[Str], value: Any]() => :CageRilla[Unit]
```

JS オブジェクトのプロパティに値を代入する descriptor を作ります。
代入操作は副作用を持ち、戻り値は `Unit` です。

- `path` は代入対象プロパティまでの経路。
- `value` は Taida 側の値。

```taida
Cage[config, JSSet[@["debug"], true]()]() ]=> _
```

Taida 側の値は不変ですが、`Cage` を介して呼び出した JS オブジェクトに
対しては副作用としての代入が許容されます。代入後の JS オブジェクトを
Taida 側で参照する場合は別途 `JSGet` で取り出してください。

---

## 4. バインド / スプレッド

### 4.1 `JSBind`

```taida
JSBind[path: @[Str]]() => :CageRilla[Molten]
```

JS のメソッドを `this` に束縛した状態で取り出す descriptor を作ります。
JS の `obj.method.bind(obj)` 相当です。戻り値は `Molten` (不透明
ハンドル) で、後続の `JSCall` に渡せます。

```taida
Cage[arr, JSBind[@["push"]]()]() ]=> pushFn
Cage[arr, JSCall[@["push"], @[item], Unit]()]()   // 等価
```

### 4.2 `JSSpread`

```taida
JSSpread[source: @[Str]]() => :CageRilla[Molten]
```

JS のスプレッド演算子 (`...source`) を呼び出し位置の引数列に展開する
descriptor を作ります。`JSCall` の `args` 内で他の値と組み合わせて
利用します。

```taida
Cage[Math, JSCall[@["max"], @[JSSpread[@["values"]]()], Int]()]() ]=> peak
```

---

## 5. バックエンド対応

| バックエンド | 対応範囲 |
|--------------|----------|
| インタプリタ | 利用不可 (JavaScript ホストが必要) |
| ネイティブ | 利用不可 |
| JS | 全シンボル対応 |
| WASM (全プロファイル) | 利用不可 |

`taida-lang/js` は **JS バックエンド専用** です。インタプリタや
ネイティブ / WASM バックエンドからこれらのシンボルを呼ぶと、決定的な
コンパイル時エラーが返ります。クロスプラットフォームで動作するコード
を書く場合は、`taida-lang/js` への依存を JS ターゲットのファイル
にのみ閉じ込めてください。

---

## 関連リファレンス

- [`README.md`](README.md) — `docs/api/` 全体の入口
- [`docs/api/prelude.md`](prelude.md) — `Cage` / `Gorillax` / `RelaxedGorillax` のメソッドとプレリュード関数
- [`docs/guide/08_error_handling.md`](../guide/08_error_handling.md) — `Gorillax` / `RelaxedGorillax` / `errorInfo` の扱い
