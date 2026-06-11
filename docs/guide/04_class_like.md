# クラスライク型定義

> **PHILOSOPHY.md — II.** だいじなものはふくろにしまっておきましょう
> **PHILOSOPHY.md — III.** カタめたいなら、鋳型を作りましょう

> 推奨読書順序: [ぶちパック構文](04_buchi_pack.md) (`04_buchi_pack.md` — 値リテラル) → 本章 (`04_class_like.md` — 型定義) の順。番号 04 を共有する 2 章は、値と型を意図的に分離しており、値リテラル `@(...)` を先に把握してから本章の型定義 (`Pilot = @(...)`) に進むと、`@(...)` 構文が値側と型側でどう対応するかが理解しやすくなります。

Taida のユーザー定義型 (構造化データ型 / モールド系統 / エラー系統) は、すべて本章のクラスライク型構文で書きます。

---

## 概要

Taida のユーザー定義型は、すべて **クラスライク型 (class-like type) の単一構文** で表現します。

```
Name[?type-args] [=> Parent] = @(field-or-method, ...)
```

| 部位 | 役割 |
|------|------|
| `Name` | 型名 (PascalCase) |
| `[?type-args]` | 型引数 (省略可、引数なしで宣言したい場合に書かない) |
| `=> Parent` | 親型からの継承 (省略可) |
| `= @(...)` | 構造定義 (フィールド・メソッド) |

構造化データの型・モールド型 (`Mold[T]` を親に持つもの)・エラー型 (`Error` を親に持つもの) は、いずれもこの単一構文で定義します。`declare-only` 関数フィールドや型引数、自動生成される `defaultFn` の挙動は、どの系統でも共通です。

---

## 基本形

### 単純なクラスライク型

```taida
Pilot = @(
  name: Str
  age: Int
  active: Bool
)

// インスタンス化
rei <= Pilot(name <= "Rei", age <= 14, active <= true)

// フィールドアクセス
rei.name   // "Rei"
```

`Pilot = @(...)` は `Pilot[] = @(...)` の省略形です。型引数を持たない場合は `[]` ごと書かずに済みます (Taida「書かなくていいものは書かない」原則)。

### 型引数を持つクラスライク型

型を抽象化したい場合は、型引数を `[T]` のように与えます。

```taida fragment
Box[T] = @(
  filling: T
  label: Str
)

intBox <= Box[Int](filling <= 42, label <= "answer")
strBox <= Box[Str](filling <= "hi", label <= "greeting")
```

型引数は単一大文字 (`T`, `U`, `V`, `E`, `K`, `P`, `R` 等) で命名するのが規則です。詳細は [命名規則](../reference/naming_conventions.md) を参照してください。

---

## フィールド区切り (カンマと改行)

クラスライク型定義 / インスタンス化 / ぶちパック値リテラル `@(...)` のいずれでも、フィールド区切りは **カンマまたは改行** で書きます。両方が同じ意味で、混在しても構いません。読み手が自然と感じる方を選んでください。

```taida fragment
// 改行区切り (型定義で多い書き方)
Pilot = @(
  name: Str
  age: Int
  active: Bool
)

// カンマ区切り (1 行に収めたいとき)
Pilot = @(name: Str, age: Int, active: Bool)

// 混在も合法
rei <= Pilot(
  name <= "Rei", age <= 14,
  active <= true
)
```

ぶちパック値リテラルとクラスライク型定義の間で区切り規則は変わりません。

> 現実装では区切り文字をまったく書かない空白区切り (`@(name: Str age: Int)`) もパースが通りますが、これは将来の仕様で禁止される可能性があります。**カンマか改行のいずれかを明示** してください。

---

## 継承

`=> Parent` で親型から継承します。`=>` の左に親型、右に子型を書きます。

```taida
Pilot = @(
  name: Str
  age: Int
)

// Pilot を継承する NervStaff
Pilot => NervStaff = @(
  department: Str
  rank: Int
)

staff <= NervStaff(
  name <= "Ritsuko",
  age <= 30,
  department <= "Science",
  rank <= 2
)

// 親フィールドにもアクセス可能
staff.name        // "Ritsuko"
staff.department  // "Science"
```

### 親型の型引数と引数の数の一致

親型に型引数がある場合、子側で **同じ数の型引数** を親型に渡す必要があります。

```taida fragment
// 親型: 2 引数
Result[T, P] = @(...)

// OK: 子側で親型を 2 引数で適用、子型自身の追加引数 V を持つ
Result[T, P] => CustomResult[T, P, V] = @(
  meta: V
)

// NG: 引数の数が合わない
// Result[T] => Bad[T] = @(...)             // 親へ 1 引数 (実際は 2 必須) → [E1407]
// Result[T, P, V] => Bad[T, P, V] = @(...) // 親へ 3 引数 (実際は 2)    → [E1407]
```

> 親型適用の引数数不一致は `[E1407]` で拒否されます。型ヘッダの引数の数 / 親型側の引数並びの保存 / 親型の種別 / 型引数の一意性をまとめてカバーする診断です。

### 子側での型引数追加

親型に渡す引数の数が合っていれば、子側で型引数を追加できます。

```taida fragment
// 親: 2 引数
CustomType[T, U] = @(a: T, b: U)

// 子で 1 つ追加: V
CustomType[T, U] => CustomSubType[T, U, V] = @(c: V)
```

---

## メソッド (関数フィールド)

クラスライク型のフィールドは、値だけでなく関数も持てます。

```taida
Pilot = @(
  name: Str
  age: Int
  intro =
    `I'm ${name}, age ${age.toString()}.`
  => :Str
)

ritsuko <= Pilot(name <= "Ritsuko", age <= 30)
greeting <= ritsuko.intro()  // "I'm Ritsuko, age 30."
```

テンプレートリテラルの `${ ... }` には式のみを書きます。`>=>` や `<=<` のような取り出し系は書けません。Int から Str への変換が必要な場合は `.toString()` を使い、Lax から取り出す必要がある場合は事前に `>=>` で束縛してからテンプレートに埋め込んでください。

メソッド内では、親フィールドも子フィールドも区別なく直接アクセスできます。`self` や `super` のような特別な識別子は不要です。

### declare-only 関数フィールド

メソッドの本体を書かず、シグネチャだけ宣言できます。これは「インターフェース」のような使い方を可能にします。

```taida
Greeter = @(
  name: Str
  greet: Str => :Str    // declare-only: 本体なし
)
```

declare-only な関数フィールドは、構造化データ型・モールド系統・エラー系統のいずれでも書けます。

declare-only 関数フィールドのデフォルト値は、**defaultFn の自動生成** によって埋められます。defaultFn は宣言したシグネチャに合わせて引数を受け取り、戻り型のデフォルト値を返す関数です。

```taida
// Str => :Str の defaultFn は引数を受け取り "" を返す
hello <= Greeter(name <= "Hi")
hello.greet("anyone")   // "" (defaultFn で自動充足)
```

戻り型が defaultFn を生成できない型 (中身を持たない不透明型や、解決できない型別名) の場合、`[E1410]` で拒否されます。

> defaultFn の詳細仕様は [関数](09_functions.md) の「defaultFn」節を参照してください。

---

## モールド系統 (操作モールド)

`Mold[T]` を親に取った class-like 型は、特に **モールド (mold) または操作モールド** と呼ばれ、値を流し込む鋳型として使われます。

```taida fragment
Mold[T] => Result[T, P <= :T => :Bool] = @(
  throw: Error
)

// 値を流し込む
ok <= Result[42, _ = true]()

// 取り出す
ok >=> value   // 42
```

モールドの取り出し挙動は `unmold` フックで決まります。詳しくは [モールド](05_mold.md) を参照してください。

> `Mold[T] =>` は特別構文ではなく、標準ライブラリで提供される基底型 `Mold[T]` から継承しているだけです。一般化された継承構文として読みます。

---

## エラー系統

エラー型もクラスライク型の単一構文で定義します。

```taida
Error => NotFound = @(
  msg: Str
  hint: Str => :Str   // declare-only な関数フィールドは許可
)

// throw / |== は通常通り
findUser id: Int =
  | id < 0 |> NotFound(msg <= "negative id").throw()
  | _      |> @(name <= "found")
=> :@(name: Str)
```

> `Error =>` も特別構文ではありません。標準ライブラリの基底型 `Error` を継承しているだけで、`Error => NotFound = @(...)` は「親型 `Error` から継承したクラスライク型 `NotFound`」と読みます。

詳しい error handling パターンは [エラー処理](08_error_handling.md) を参照してください。

---

## 構造的部分型付け

Taida は構造的部分型付け (structural subtyping) を採用しています。クラスライク型でも、必要なフィールドを持っていれば互換と見なされます。

```taida fragment
HasName = @(name: Str)

greet person: HasName =
  stdout("Hello, " + person.name)
=> :Int

// Pilot は name フィールドを持つので、HasName として渡せる
pilot <= Pilot(name <= "Asuka", age <= 14)
greet(pilot)   // "Hello, Asuka" を出力し、書き込んだバイト数 (Int) を返します。戻り値は破棄して構いません。
```

`stdout` は書き込んだ UTF-8 バイト数を `:Int` で返すプレリュード関数です。Taida には「値を返さない型」が存在しません。すべての関数は必ず何らかの型を返します。戻り値を使わない場合でも、シグネチャ上は `:Int` を明示します。

---

## デフォルト値

クラスライク型の各フィールドは、型に応じたデフォルト値を持ちます。インスタンス化時に省略すると、デフォルト値が使われます。

```taida
Pilot = @(name: Str, call_sign: Str, age: Int)

rei <= Pilot(name <= "Rei")
// rei.call_sign == ""
// rei.age == 0
```

すべての型にデフォルト値が保証されている (`null` / `undefined` の排除) のは Taida の根本哲学です。関数型のフィールドについても、**defaultFn が自動生成** されるため、必ず値が存在します。

---

## コンストラクタの closed-form 検査

`Name(field <= value, ...)` のクラスライク型コンストラクタ呼び出しは
**closed-constructor** として検査されます。匿名 `@(...)` リテラルは引き
続き open / 構造的な値として扱われ、本ポリシーの対象外です。

| ケース | 受理 / 拒否 | 診断 |
|---|---|---|
| 宣言済みフィールドのみで呼び出し | 受理 | — |
| 宣言済みフィールドの省略 | 受理 (型のデフォルト値で充足) | — |
| declare-only 関数フィールドの省略 | 受理 (型の defaultFn が充足) | — |
| 宣言外フィールドを渡す | 拒否 | `[E1406]` |
| 同一フィールドを複数回渡す | 拒否 | `[E1404]` |
| 宣言済みフィールドに型不一致な値を渡す | 拒否 | `[E1506]` |
| メソッドフィールドに値を渡す | 拒否 | `[E1407]` |
| Error 派生で `type` に同名 literal を渡す | 受理 | — |
| Error 派生で `type` に別 literal / 非 literal を渡す | 拒否 | `[E1408]` |
| 匿名 `@(...)` への余分フィールド | 受理 (open shape を維持) | — |

```taida fragment
Pilot = @(name: Str, age: Int)

Pilot(name <= "Rei", age <= 14)            // OK
Pilot(name <= "Asuka")                     // OK (age はデフォルト 0)
Pilot(name <= "Rei", typo_age <= 14)       // [E1406]
Pilot(name <= "Rei", name <= "Asuka")      // [E1404]
Pilot(name <= "Rei", age <= "fourteen")    // [E1506]

@(x <= 1, y <= "extra", z <= true)         // OK (匿名 pack は open)

Error => MyError = @(field: Str, message: Str)
MyError(type <= "MyError", field <= "x", message <= "y")  // OK
MyError(type <= "AppError", message <= "y")               // [E1408]
MyError(feild <= "typo", message <= "y")                  // [E1406]
```

構造的サブタイピングは代入 / 引数渡し / 戻り値の互換規則として維持されます。
コンストラクタの閉形式検査は、型の同一性を立てる際のフィールド名の打ち間違い検出と、
気付かれずに壊れた呼び出しの抑止だけを担当します。構造的互換性の規則とは独立に動きます。

### 継承時のフィールド再定義

子クラスライク型が親と同名のフィールドを宣言し、型が **同一** であれば
冪等として受理されます。同名で型が異なる再定義は `[E1411]` で拒否されます。

```taida fragment
Pilot = @(name: Str, age: Int)
Pilot => Operator = @(name: Str)              // OK: 同名同型 (冪等)
Pilot => BrokenOp = @(name: Int)              // [E1411] 同名異型
```

---

## ネスト構造

class-like 型の中に class-like 型を埋め込めます。

```taida
Pilot = @(
  name: Str
  contact: @(
    email: Str
    phone: Str
  )
)

shinji <= Pilot(
  name <= "Shinji",
  contact <= @(email <= "shinji@nerv.jp", phone <= "NERV-002")
)

shinji.contact.email   // "shinji@nerv.jp"
```

---

## まとめ

| 概念 | 構文 |
|------|------|
| 値の作成 (リテラル) | `@(field <= value, ...)` (詳細は [ぶちパック構文](04_buchi_pack.md)) |
| クラスライク型定義 | `Name[?type-args] [=> Parent] = @(...)` |
| インスタンス化 | `Name[type-args](field <= value, ...)` (引数省略可) |
| フィールドアクセス | `instance.fieldName` |
| メソッド呼び出し | `instance.methodName(args)` |
| declare-only 関数フィールド | `name: ArgType => :ReturnType` (本体なし、defaultFn で自動充足) |
| モールド系統 | `Mold[T] => Foo[T] = @(...)` (操作モールド) |
| エラー系統 | `Error => NotFound = @(...)` (`Error` を継承するクラスライク型) |
| 親型適用の引数数不一致 | `[E1407]` |
| declare-only フィールドのデフォルト生成不可 | `[E1410]` |
| 継承時の同名異型再定義 | `[E1411]` |
| 宣言外フィールドのコンストラクタ呼び出し | `[E1406]` |
| 同名フィールドの重複指定 | `[E1404]` |
| Error 派生 `type` への不一致 literal | `[E1408]` |

---

## 関連ドキュメント

- [リテラル `@(...)` / `@[...]`](04_buchi_pack.md) — 値リテラル
- [モールド (Mold)](05_mold.md) — `unmold` フック
- [エラー処理](08_error_handling.md) — Lax / throw / `|==` / Gorillax
- [関数](09_functions.md) — defaultFn 仕様
- [診断コード](../reference/diagnostic_codes.md) — `[E1407]` / `[E1410]` 等
- [命名規則](../reference/naming_conventions.md) — 型名と型引数の命名
