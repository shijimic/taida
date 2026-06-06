# 型制約

Taida の型制約は、ジェネリック関数やモールドの型変数に対して、
受け取れる型の範囲を明示します。制約は暗黙変換を行いません。呼び出し側の型が
制約を満たさない場合、型チェッカーが診断を出します。

```taida
idInt[T <= :Int] value: T =
  value
=> :T

Mold[T <= :Int] => IntBox[T <= :Int] = @(marker: Int <= 0)
```

## `Wired[T]`

`Wired[T]` は、値を host wire 形式へ構造を保ったまま渡せることを表す制約です。
関数値、`Async[T]`、未確定の `Lax[T]`、`@()` 型は wire 値として扱いません。

```taida
wireId[T <= :Wired[T]] value: T =
  value
=> :T

Mold[T <= :Wired[T]] => WireBox[T <= :Wired[T]] = @(marker: Int <= 0)
```

`Wired[T]` を満たす型:

| 型 | 条件 |
|----|------|
| `Str` / `Int` / `Float` / `Bool` / `Bytes` | そのまま wire 値になります。 |
| `@[U]` | `U` が `Wired[U]` を満たす場合。空リストも、要素型が注釈などで確定していれば有効です。 |
| `@(f: U, ...)` | フィールドが 1 つ以上あり、全フィールド型が `Wired` を満たす場合。 |
| 名前付きぶちパック型 | 展開後のフィールドがすべて `Wired` を満たす場合。 |
| `WebRequest` / `WebResponse` | `taida-lang/abi` の定義済み host boundary 型です。 |
| `HostCapability[Name, Kind]` | host capability 参照を表す値です。 |

`Num` は wire 上の独立した実値型ではないため、`Int` または `Float` に確定させて
から渡します。

Host capability では、`HostStep[method, args]()` の `args` が `Wired` な値の
リストであることを要求します。`Bytes` は wire 上で標準 base64 文字列になり、
`WebRequest` / `WebResponse` は handler ABI と同じ `bodyBase64` 形で運ばれます。

`Wired[T]` 違反は `[E3601]` です。`HostCall` の steps list が `HostStep` 以外を
含む場合は `[E3602]`、host boundary descriptor の compile-time identity や
manifest 照合に失敗した場合は `[E3603]` です。
