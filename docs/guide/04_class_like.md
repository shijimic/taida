# クラスライク型定義

> **PHILOSOPHY.md -- II.** だいじなものはふくろにしまっておきましょう
> **PHILOSOPHY.md -- III.** カタめたいなら、鋳型を作りましょう

> **E30 (gen-E 破壊的変更) で導入された統一構文。** 旧 3 系統 (TypeDef / Mold 継承 / Error 継承) は本章の単一構文に統合されました。旧構文からの移行は [migration_e30.md](migration_e30.md) を参照してください。

---

## 概要

Taida のユーザー定義型は、すべて **クラスライク型 (class-like type) の単一構文** で表現します。

```
Name[?type-args] [=> Parent] = @(field-or-method, ...)
```

| 部位 | 役割 |
|------|------|
| `Name` | 型名 (PascalCase) |
| `[?type-args]` | 型引数 (省略可、zero-arity sugar) |
| `=> Parent` | 親型からの継承 (省略可) |
| `= @(...)` | 構造定義 (フィールド・メソッド) |

旧 D 世代までは TypeDef (ぶちパック型定義) / Mold 継承 / Error 継承の 3 系統が独立した surface 構文を持っていましたが、E30 で 1 つに統合されました。能力 (declare-only 関数フィールドの可否、型変数の有無、defaultFn 自動生成) はすべての系統で共通です。

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

`Pilot = @(...)` は `Pilot[] = @(...)` の **zero-arity sugar** であり、型引数を持たない場合は `[]` を省略できます (Taida「書かなくていいものは書かない」原則)。

### 型引数を持つクラスライク型

型を抽象化したい場合は、型引数を `[T]` のように与えます。

```taida
Box[T] = @(
  filling: T
  label: Str
)

intBox <= Box[Int](filling <= 42, label <= "answer")
strBox <= Box[Str](filling <= "hi", label <= "greeting")
```

型引数は単一大文字 (`T`, `U`, `V`, `E`, `K`, `P`, `R` 等) で命名するのが規則です。詳細は [命名規則](../reference/naming_conventions.md) を参照してください。

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

### 親型の型引数と arity 一致

親型に型引数がある場合、子側で **arity を一致** させて適用する必要があります。

```taida
// 親型: 2 引数
Result[T, P] = @(...)

// OK: 子側で親型を 2 引数で適用、子型自身の追加引数 V を持つ
Result[T, P] => CustomResult[T, P, V] = @(
  meta: V
)

// NG: arity mismatch
// Result[T] => Bad[T] = @(...)        // 親へ 1 引数 (実際は 2 必須) → [E1407]
// Result[T, P, V] => Bad[T, P, V] = @(...)  // 親へ 3 引数 (実際は 2) → [E1407]
```

> 親型適用の arity 不一致は `[E1407]` で reject されます (header arity / prefix preservation / 親種別 / type param uniqueness を含む umbrella)。

### 子側での型引数追加

親型 arity が一致していれば、子側で型引数を追加できます。

```taida
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
    `I'm ${name}, age ${Str[age]() ]=> _}.`
  => :Str
)

ritsuko <= Pilot(name <= "Ritsuko", age <= 30)
greeting <= ritsuko.intro()  // "I'm Ritsuko, age 30."
```

メソッド内では、親フィールドも子フィールドも区別なく直接アクセスできます。`self` や `super` のような特別な識別子は不要です。

### declare-only 関数フィールド

メソッドの本体を書かず、シグネチャだけ宣言できます。これは「インターフェース」のような使い方を可能にします。

```taida
Greeter = @(
  name: Str
  greet: Str => :Str    // declare-only: 本体なし
)
```

declare-only 関数フィールドは、E30 で **すべての系統 (旧 TypeDef / Mold 継承 / Error 継承) で許可** されます (E30B-002)。

declare-only 関数フィールドの default 値は、E30 で導入される **defaultFn 自動生成** によって充足されます。defaultFn は引数を受け取り、戻り型のデフォルト値を返す関数です。

```taida
// Str => :Str の defaultFn は引数を受け取り "" を返す
hello <= Greeter(name <= "Hi")
hello.greet("anyone")   // "" (defaultFn で自動充足)
```

戻り型が defaultFn を生成できない型 (opaque / abstract external type) の場合、`[E1410]` で reject されます。

> defaultFn の詳細仕様は [関数](09_functions.md) の「defaultFn」節を参照してください (E30 Phase 6 で実装と同期して追加されます)。

---

## モールド系統 (操作モールド)

`Mold[T]` を親に取った class-like 型は、特に **モールド (mold) または操作モールド** と呼ばれ、値を流し込む鋳型として使われます。

```taida
Mold[T] => Result[T, P <= :T => :Bool] = @(
  throw: Error
)

// 値を流し込む
ok <= Result[42, _ = true]()

// 取り出す
ok ]=> value   // 42
```

モールドは `solidify` / `unmold` フックで挙動が決まります。詳しくは [モールディング型 (操作モールド)](05_molding.md) を参照してください。

> E30 では `Mold[T] =>` という prefix を **任意の type 名から派生する一般化された継承構文** として扱います。Mold は単に標準ライブラリで提供される base 型のひとつです。

---

## エラー系統

エラー型も class-like 単一構文で定義します。

```taida
Error => NotFound = @(
  msg: Str
  recovery: Unit => :Unit  // declare-only 関数フィールド OK (E30B-002)
)

// throw / |== は通常通り
findUser id: Int =
  | id < 0 |> NotFound(msg <= "negative id").throw()
  | _      |> @(name <= "found")
=> :@(name: Str)
```

> E30 では `Error =>` という prefix を **特別構文として扱わず、通常の class-like 継承** として解釈します。Error は標準ライブラリで提供される base 型のひとつです。`Error => NotFound = @(...)` は「親型 Error から継承した class-like 型 NotFound」と読みます。

詳しい error handling パターンは [エラー処理](08_error_handling.md) を参照してください。

---

## 構造的部分型付け

Taida は構造的部分型付け (structural subtyping) を採用しています。class-like 型でも、必要なフィールドを持っていれば互換と見なされます。

```taida
HasName = @(name: Str)

greet person: HasName =
  stdout("Hello, " + person.name)
=> :Void

// Pilot は name フィールドを持つので、HasName として渡せる
pilot <= Pilot(name <= "Asuka", age <= 14)
greet(pilot)   // "Hello, Asuka"
```

この性質は旧 3 系統で共通だったもので、E30 でも維持されます。

---

## デフォルト値

クラスライク型の各フィールドは、型に応じたデフォルト値を持ちます。インスタンス化時に省略すると、デフォルト値が使われます。

```taida
Pilot = @(name: Str, call_sign: Str, age: Int)

rei <= Pilot(name <= "Rei")
// rei.call_sign == ""
// rei.age == 0
```

すべての型にデフォルト値が保証されている (`null` / `undefined` の排除) のは Taida の根本哲学です。関数型のフィールドについても、E30 から **defaultFn が自動生成** され、必ず値が存在することが保証されます。

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

## 旧構文との対応 (D29 まで)

D29 までは 3 系統が独立した surface 構文を持っていました。

| 旧系統 | 旧構文 | 新統一構文 |
|--------|--------|-----------|
| TypeDef | `Pilot = @(name: Str)` | `Pilot = @(name: Str)` (zero-arity sugar として保持) |
| TypeDef 継承 | `Pilot => NervStaff = @(...)` | `Pilot => NervStaff = @(...)` (継承構文として保持) |
| Mold 継承 | `Mold[T] => Foo[T] = @(...)` | `Mold[T] => Foo[T] = @(...)` (一般化された継承として保持) |
| Error 継承 | `Error => NotFound = @(...)` | `Error => NotFound = @(...)` (一般化された継承として保持) |

**surface はほぼ変わりません**。変わるのは「3 系統が別々のもの」という概念の取り扱いです。E30 以降は、すべて **class-like 単一概念** として読みます。

旧コードからの移行手順は [migration_e30.md](migration_e30.md) を参照してください。

---

## 移行について

`@e.X` 以降の CLI には移行用ハブはありません。`@e.30` への移行は
[migration_e30.md](migration_e30.md) のチェックリストに従って手動で行います。

```bash
taida way check <PATH>
# Run your project-specific tests after the Taida gate passes.
```

古い RC 計画やメモにある「移行コマンド」案は `@e.X` の確定 CLI には含まれません。

> gen-E は **予告期間なし、即破壊的変更** で確定しています（E30 Lock-E）。旧構文を使うと `@e.30` から `[E14xx]` 系の診断で拒否されます。

---

## まとめ

| 概念 | 構文 |
|------|------|
| 値の作成 (リテラル) | `@(field <= value, ...)` (詳細は [04_pack_literal](04_buchi_pack.md)) |
| クラスライク型定義 | `Name[?type-args] [=> Parent] = @(...)` |
| インスタンス化 | `Name[type-args](field <= value, ...)` (引数省略可) |
| フィールドアクセス | `instance.fieldName` |
| メソッド呼び出し | `instance.methodName(args)` |
| declare-only 関数フィールド | `name: ArgType => :ReturnType` (本体なし、defaultFn で充足) |
| モールド系統 | `Mold[T] => Foo[T] = @(...)` (操作モールド) |
| エラー系統 | `Error => NotFound = @(...)` (一般化継承) |
| 親型適用 arity mismatch | `[E1407]` (header arity / prefix preservation / 親種別 / type param uniqueness を含む umbrella) |
| declare-only field の default 不能 | `[E1410]` (戻り型が opaque / unknown alias で defaultFn 生成不可、definition-site で発火) |

---

## 関連ドキュメント

- [リテラル `@(...)` / `@[...]`](04_buchi_pack.md) — 値リテラル中心
- [操作モールド (Mold)](05_molding.md) — `solidify` / `unmold` フック
- [エラー処理](08_error_handling.md) — Lax / throw / `\|==` / Gorillax
- [関数](09_functions.md) — defaultFn 仕様 (Phase 6 で追記)
- [migration_e30](migration_e30.md) — 旧構文 → 新統一構文 移行ガイド
- [診断コード](../reference/diagnostic_codes.md) — `[E1407]` / `[E1410]` 等
- [命名規則](../reference/naming_conventions.md) — 型名 / 型引数の命名
