# 標準ライブラリリファレンス

## 概要

Taida は**プリリュード**と**ビルトイン型**で構成されています。全てインポート不要で使用できます。

> **PHILOSOPHY.md — I.** 深く考えずに適当にぶちこんでけ

操作系の処理はモールドとして提供されます。詳細は `reference/mold_types.md` を参照してください。
メソッドは状態チェックとモナディック操作に限定されています。詳細は `reference/standard_methods.md` を参照してください。
Prelude の追加可否基準は `docs/design/prelude_boundary.md` を参照してください。

---

## プリリュード関数

インポート不要で使用できる関数です。

### 入出力

| 関数 | 説明 | 例 |
|------|------|-----|
| `stdout(msg)` | 標準出力に出力し、書き込んだ UTF-8 バイト数を返す（`Int`） | `bytes <= stdout("作戦開始")` |
| `stderr(msg)` | 標準エラー出力に出力し、書き込んだ UTF-8 バイト数を返す（`Int`） | `bytes <= stderr("警告: 残弾なし")` |
| `stdin(prompt?)` | プロンプトを表示して入力を受け取る（`Str`。EOF / IO error で `""`） | `name <= stdin("名前: ")` |
| `stdinLine(prompt?)` | UTF-8-aware 対話入力（`Async[Lax[Str]]`、`]=>` で unmold、C20-2 以降） | `stdinLine("名前: ") ]=> name` |
| `nowMs()` | 現在時刻（epoch ミリ秒）を取得（`Int`） | `start <= nowMs()` |
| `sleep(ms)` | 指定ミリ秒待機する（`Async[Unit]`） | `sleep(10) ]=> _done` |

```taida
// Misato の日常業務
stdout("NERV 作戦部、葛城ミサトです")
stderr("警告: エヴァ初号機の同期率が低下しています")
name <= stdin("オペレーター名を入力: ")
stdout("了解、" + name + "。作戦を開始します")

start <= nowMs()
wait <= sleep(10)
wait ]=> _done
end <= nowMs()
stdout((end - start).toString())
```

#### `stdout` / `stderr` 戻り値（C12 以降）

`stdout(...)` と `stderr(...)` は書き込んだペイロードの UTF-8 バイト数を
`Int` で返します（末尾の改行は含まない）。

```taida
// 書き込みバイト数を受け取る
bytes <= stdout("hello")   // bytes = 5
stdout(bytes)              // "5"

// 従来どおり、文として使う場合は値は自然に破棄される
stdout("そのまま動く")     // 値を受け取らない書き方も互換
```

`Value::Unit` は Taida プログラムからは観測できません。これは
PHILOSOPHY I（「null/undefined の完全排除」）に沿った設計です
— 副作用関数もデフォルト値を持つ `Int` を返し、常に何かを返す
という不変条件を保ちます。

### JSON シリアライズ

| 関数 | 説明 | 例 |
|------|------|-----|
| `jsonEncode(value)` | ぶちパックを JSON 文字列に変換 | `jsonEncode(data)` |
| `jsonPretty(value)` | ぶちパックを整形 JSON 文字列に変換 | `jsonPretty(data)` |

```taida
pilot <= @(name <= "Misato", age <= 29, role <= "Operations Director")
jsonEncode(pilot)   // `{"name":"Misato","age":29,"role":"Operations Director"}`
jsonPretty(pilot)
// {
//   "name": "Misato",
//   "age": 29,
//   "role": "Operations Director"
// }
```

> **注意**: JSON のパース（外部データ → Taida 値）には `JSON[raw, Schema]()` モールドを使用します。詳細は `docs/design/json_molten_iron.md` を参照してください。

### ユーティリティ

| 関数 | 説明 | 例 |
|------|------|-----|
| `typeof(x)` | 型名を文字列で返す | `typeof(42)` → `"Int"` |
| `range(start, end)` | 整数リスト生成 | `range(0, 5)` → `@[0, 1, 2, 3, 4]` |
| `debug(x)` | デバッグ出力して値をそのまま返す | `5 => debug => result` |

```taida
// debug はパイプラインの途中に挿入できます
scores <= @[95, 82, 78, 91]
scores => Filter[_, _ x = x > 80]() => debug => Map[_, _ x = x * 2]() => result
// debug で中間結果が stderr に出力されます

>>> taida-lang/crypto@a.1 => @(sha256)
token <= sha256("abc")
stdout(token)  // ba7816bf8f01cfea...f20015ad
```

`taida-lang/crypto` の拡張契約（HMAC/KDF/安全乱数/署名検証）は `docs/design/crypto_package_contract.md` を参照してください。

---

## Core Builtins（数値・バイト列）

インポート不要の組み込み型/モールドです。`Bytes` は Prelude 関数ではなく Core Builtin です。

### 型

| 型 | デフォルト値 | 用途 |
|---|---|---|
| `Int` | `0` | 64bit 整数 |
| `Float` | `0.0` | 浮動小数点 |
| `Bytes` | `Bytes[@[]]` | 0..255 のバイト列 |

### 数値拡張モールド

| モールド | 戻り値 | 説明 |
|---|---|---|
| `BitAnd[a, b]()` / `BitOr[a, b]()` / `BitXor[a, b]()` / `BitNot[x]()` | `Int` | ビット演算 |
| `ShiftL[x, n]()` / `ShiftR[x, n]()` / `ShiftRU[x, n]()` | `Lax[Int]` | シフト（`n` は 0..63 で成功） |
| `ToRadix[int, base]()` | `Lax[Str]` | `base`（2..36）への基数変換 |
| `Int[str, base]()` | `Lax[Int]` | 指定基数の文字列を整数化 |

> ビット演算子は追加しません。上記モールドを使用します。

### バイト列・Unicode モールド

| モールド | 戻り値 | 説明 |
|---|---|---|
| `UInt8[x]()` | `Lax[Int]` | `0..255` 制約付き変換 |
| `Bytes[x](fill <= n)` | `Lax[Bytes]` | `Int/Str/@[Int]/Bytes` から Bytes へ変換 |
| `ByteSet[bytes, idx, value]()` | `Lax[Bytes]` | 指定位置更新（不変） |
| `BytesToList[bytes]()` | `@[Int]` | Bytes を整数リストへ変換 |
| `BytesCursor[bytes](offset <= n)` | `@(bytes: Bytes, offset: Int, length: Int)` | Bytes の順次読み取りカーソルを生成 |
| `BytesCursorRemaining[cursor]()` | `Int` | 残り読み取り可能バイト数を返す |
| `BytesCursorTake[cursor, size]()` | `Lax[@(value: Bytes, cursor: @(...))]` | `size` バイト読み取り + カーソル前進 |
| `BytesCursorU8[cursor]()` | `Lax[@(value: Int, cursor: @(...))]` | 1バイト読み取り（`0..255`）+ カーソル前進 |
| `U16BE[x]()` / `U16LE[x]()` | `Lax[Bytes]` | 16bit 符号なし整数を endian 指定で2バイトへパック |
| `U32BE[x]()` / `U32LE[x]()` | `Lax[Bytes]` | 32bit 符号なし整数を endian 指定で4バイトへパック |
| `U16BEDecode[bytes]()` / `U16LEDecode[bytes]()` | `Lax[Int]` | 2バイトを endian 指定で 16bit 整数へデコード |
| `U32BEDecode[bytes]()` / `U32LEDecode[bytes]()` | `Lax[Int]` | 4バイトを endian 指定で 32bit 整数へデコード |
| `Char[x]()` | `Lax[Str]` | 1文字への変換 |
| `CodePoint[str]()` | `Lax[Int]` | 1文字のコードポイント取得 |
| `Utf8Encode[str]()` | `Lax[Bytes]` | UTF-8 エンコード |
| `Utf8Decode[bytes]()` | `Lax[Str]` | UTF-8 デコード（不正列は failure） |

`UnsafePointer` はコア言語に導入しません。必要な低レベル連携は不透明ハンドル（`Molten`）+ 専用APIで隔離します。

---

## プリリュード型コンストラクタ

インポート不要で使用できる型コンストラクタです。

### Result

述語付き操作モールドです。述語 P（`:T => :Bool`）で成功/失敗を判定します。`]=>` で述語を評価し、真なら値 T、偽なら throw が発動します。

| 構文 | 説明 | 例 |
|------|------|-----|
| `Result[value, _ = true]()` | 常に成功の Result | `Result[42, _ = true]()` |
| `Result[value, _ = false](throw <= error)` | 常に失敗の Result | `Result[0, _ = false](throw <= err)` |
| `Result[value, pred](throw <= error)` | 述語で判定する Result | `Result[age, _ x = x >= 18](throw <= err)` |

```taida
Error => ValidationError = @(message: Str)

validateAge age: Int =
  Result[age, _ x = x > 0](throw <= ValidationError(type <= "ValidationError", message <= "年齢は正の値でなければなりません"))
=> :Result[Int, _]
```


### Gorillax / Cage

覚悟のモールド型です。unmold 失敗時にデフォルト値ではなくゴリラ（プログラム即終了）が発動します。

| 構文 | 説明 | 例 |
|------|------|-----|
| `Gorillax[value]()` | 値を Gorillax で包む | `Gorillax[42]()` |
| `Cage[molten, fn]()` | Molten 専用。fn(molten) を実行し Gorillax で包む | `Cage[lodash, _ lo = lo.sum(items)]()` |

```taida
// Cage で溶鉄に操作 → Gorillax で受け取る
Cage[externalLib, _ lib = lib.process(data)] => result
result ]=> value         // 成功 → 値, 失敗 → ゴリラ

// .relax() で |== キャッチ可能に変換
result.relax() => relaxed  // relaxed: RelaxedGorillax[T]
```

### コレクション

| コンストラクタ | 説明 | 例 |
|--------------|------|-----|
| `hashMap()` | 空の HashMap を生成 | `hashMap()` |
| `setOf(list)` | リストから Set を生成 | `setOf(@[1, 2, 3])` |

---

## HashMap メソッド

HashMap はイミュータブルです。変更操作は新しい HashMap を返します。

```taida
pilots <= hashMap()
  .set("Misato", @(age <= 29, role <= "Operations Director"))
  .set("Ritsuko", @(age <= 30, role <= "Chief Scientist"))
```

| メソッド | 戻り値 | 説明 |
|---------|--------|------|
| `.get(key)` | `Lax[V]` | キーに対応する値を取得 |
| `.set(key, value)` | `HashMap[K, V]` | キーと値を追加した新しい HashMap |
| `.remove(key)` | `HashMap[K, V]` | キーを削除した新しい HashMap |
| `.has(key)` | `Bool` | キーが存在するか |
| `.keys()` | `@[K]` | キーのリスト |
| `.values()` | `@[V]` | 値のリスト |
| `.entries()` | `@[@(key, value)]` | キーと値のペアのリスト |
| `.size()` | `Int` | エントリ数 |
| `.merge(other)` | `HashMap[K, V]` | 2つの HashMap を結合した新しい HashMap |
| `.isEmpty()` | `Bool` | 空かどうか |
| `.toString()` | `Str` | 文字列表現 |

```taida
// get は Lax を返します
pilots.get("Misato").hasValue  // true
pilots.get("Gendo").hasValue   // false

// イミュータブルなので元の HashMap は変化しません
updated <= pilots.set("Shinji", @(age <= 14, role <= "Pilot"))
pilots.has("Shinji")   // false
updated.has("Shinji")  // true

// merge で結合
staff <= pilots.merge(updated)
```

---

## Set メソッド

Set もイミュータブルです。変更操作は新しい Set を返します。

```taida
pilotNames <= setOf(@["Misato", "Ritsuko", "Shinji"])
```

| メソッド | 戻り値 | 説明 |
|---------|--------|------|
| `.add(item)` | `Set[T]` | 要素を追加した新しい Set |
| `.remove(item)` | `Set[T]` | 要素を削除した新しい Set |
| `.has(item)` | `Bool` | 要素が含まれているか |
| `.union(other)` | `Set[T]` | 和集合 |
| `.intersect(other)` | `Set[T]` | 積集合 |
| `.diff(other)` | `Set[T]` | 差集合 |
| `.toList()` | `@[T]` | リストに変換 |
| `.size()` | `Int` | 要素数 |
| `.isEmpty()` | `Bool` | 空かどうか |
| `.toString()` | `Str` | 文字列表現 |

```taida
bridge <= setOf(@["Misato", "Ritsuko", "Maya"])
pilots <= setOf(@["Shinji", "Rei", "Asuka"])

// 集合演算
all <= bridge.union(pilots)
all.size()  // 6

bridge.has("Misato")  // true
bridge.has("Shinji")  // false

// 差集合で「パイロットでないスタッフ」を取得
staff <= all.diff(pilots)
```

