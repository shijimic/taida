# プレリュード関数リファレンス

プレリュードはインポート不要で常に利用可能な関数群です。
入出力、時間取得、JSON シリアライズ、デバッグ出力、整数列生成、プロセス
終了などの基本ユーティリティを提供します。

すべての関数のシグネチャは Taida 流に「全ての引数にデフォルト値を持つ」
前提で書かれています。引数を省略した場合は、その型のデフォルト値
(`Int` なら `0`、`Str` なら `""`) で呼び出された扱いになります。例外的に
省略時の挙動が型デフォルトと異なる関数は本書内で明示します。

インポート不要で使えるモールドは本書 §7 に一覧、ビルトイン型のメソッドは
§8 に、コレクション (HashMap / Set) は §9 にあります。モールドの解剖と
概念は [`docs/guide/05_mold.md`](../guide/05_mold.md) を参照してください。

---

## 1. 入出力

### 1.1 `stdout`

> 標準出力に値を書き出し、書き込んだ UTF-8 バイト数を返す。

```taida fragment
stdout value: Str => :Int
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `value` | `Str` | 出力する文字列。`Str` 以外の型 (`Int` / `Float` / `Bool` / ぶちパック / リスト等) も自動 stringify されます。 |

**Returns**: `:Int` — 書き込んだ UTF-8 バイト数 (末尾の自動改行は含まない)。

**AI-Context**:
各バックエンドで暗黙の改行 `\n` が末尾に付加されます。パイプ下流が
閉じている場合 (`SIGPIPE` 相当) はサイレントに `0` を返し、プロセスは
`exit 0` で終了します。

**Example**:

```taida
bytes <= stdout("Hello")        // bytes = 5
stdout(42)                      // 出力: "42"
```

### 1.2 `stderr`

> 標準エラー出力に値を書き出し、書き込んだ UTF-8 バイト数を返す。

```taida fragment
stderr value: Str => :Int
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `value` | `Str` | 出力する文字列。型変換ルールは `stdout` と同じ。 |

**Returns**: `:Int` — 書き込んだ UTF-8 バイト数。

**AI-Context**:
`stdout` と異なり、REPL / Rust 側 in-process テスト API でも呼び出し
ごとに即時 flush されます (バッファリングしません)。

### 1.3 `stdin`

> 標準入力から 1 行を読み取って返す。

```taida fragment
stdin prompt: Str => :Str
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `prompt` | `Str` | 読み取り前に標準出力へ表示する文字列。デフォルト値 `""` の場合は何も表示しない。 |

**Returns**: `:Str` — 読み取った 1 行 (改行コードは自動で除去)。EOF /
IO エラー時はサイレントに空文字列 `""` を返す。エラーを検知したい場合は
`stdinLine` を使う。

**AI-Context**:
ASCII 入力やパイプ用途を想定しています。マルチバイト編集 (Backspace で
UTF-8 コードポイント単位を扱う等) は `stdinLine` 側に対応があります。

**Example**:

```taida
name <= stdin("お名前: ")
stdout("こんにちは、" + name)
```

### 1.4 `stdinLine`

> UTF-8 対応ライン編集で標準入力から 1 行を読み取る。

```taida fragment
stdinLine prompt: Str => :Async[Lax[Str]]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `prompt` | `Str` | 読み取り前に表示する文字列。`""` で表示なし。 |

**Returns**: `:Async[Lax[Str]]` — `>=>` で待ち、さらに `>=>` で
`Lax[Str]` を展開します。EOF / Ctrl-C / Ctrl-D / IO エラー時は
`Lax.failure` を返します。

**AI-Context**:
Backspace でマルチバイト 1 文字単位の削除、Ctrl-C / Ctrl-D による中断
検知をサポートします。

**Example**:

```taida
stdinLine("名前: ") >=> name_lax
name <= name_lax.getOrDefault("ゲスト")
```

---

## 2. 時間

### 2.1 `nowMs`

> Unix epoch からの経過ミリ秒数を返す。

```taida fragment
nowMs => :Int
```

**Returns**: `:Int` — Unix epoch (1970-01-01T00:00:00Z) からの経過ミリ
秒数。

**AI-Context**:
ウォールクロックであり、単調時計ではありません。NTP 補正や手動時刻
変更でジャンプ・巻き戻しが発生する可能性があります。経過時間の厳密
測定 (タイムアウト・レート制御・パフォーマンス計測) で使う場合は許容
誤差を併用してください。正式バックエンドすべてで同じウォールクロック
契約に従います。解像度は OS / ホスト依存です。

**Example**:

```taida
start <= nowMs()
sleep(10) >=> slept
end <= nowMs()
stdout((end - start).toString())   // 例: "10"
```

### 2.2 `sleep`

> 指定ミリ秒の待機を行う非同期処理を返す。

```taida fragment
sleep ms: Int => :Async[Int]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `ms` | `Int` | 待機ミリ秒数。`0` 以下は即座に完了 (待機なし)。`Int` の範囲は `0..=2_147_483_647`。範囲外は rejected `Async` になる。 |

**Returns**: `:Async[Int]` — `>=>` で展開すると待機が完了し、実際に待機した
ミリ秒数 (基本的には `ms` と同値) が解決値として得られます。Taida は
`@()` / `:Unit` を「値の不在」型として認めないため、待機時間という意味
ある値を `:Int` として返します。

**AI-Context**:
バックエンドごとに OS / ホストランタイム提供のスリープに委譲します。

**Example**:

```taida
sleep(100) >=> elapsedMs
stdout("100ms 経過 (実測: " + elapsedMs.toString() + "ms)")
```

---

## 3. JSON シリアライズ

### 3.1 `jsonEncode`

> ぶちパック / リスト / プリミティブを JSON 文字列に変換する。

```taida fragment
jsonEncode value: T => :Str
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `value` | `T` | 変換対象。`T` はぶちパック / リスト / プリミティブ / `Lax` などの任意型。 |

**Returns**: `:Str` — 1 行で最小化された JSON 文字列 (空白を含まない)。

**AI-Context**:
ぶちパックのキーは Taida 識別子のまま JSON キーになります。内部
フィールド名 (`has_value` / `__value` 等) はそのまま出力されます。
`Lax` / `Gorillax` / `Result` は内部表現がそのまま JSON 化されます。

**Example**:

```taida
pilot <= @(name <= "Misato", age <= 29)
stdout(jsonEncode(pilot))
// 出力: {"name":"Misato","age":29}
```

### 3.2 `jsonPretty`

> `jsonEncode` と同じ入力を整形 JSON 文字列に変換する。

```taida fragment
jsonPretty value: T => :Str
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `value` | `T` | 変換対象。 |

**Returns**: `:Str` — 2 スペース固定で整形された JSON 文字列。

**Example**:

```taida
stdout(jsonPretty(@(name <= "Misato", age <= 29)))
// 出力:
// {
//   "name": "Misato",
//   "age": 29
// }
```

JSON のパース (外部データ → Taida 値) は `JSON[raw, Schema]()` モールド
を使います。詳細は [`docs/guide/03_json.md`](../guide/03_json.md) を
参照してください。

---

## 4. デバッグ

### 4.1 `debug`

> 値を標準出力に表示し、その値をそのまま返す。

```taida fragment
debug value: T => :T
debug value: T  label: Str => :T
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `value` | `T` | 表示する値。型はそのまま戻り値に伝播。 |
| `label` | `Str` | 表示時のラベル。デフォルト値 `""` の場合はラベル無し、指定時は `"label: <value>"` 形式。 |

**Returns**: `:T` — 入力された `value` をそのまま返す (副作用付き恒等
関数)。

**AI-Context**:
パイプラインの途中に挿入できる「副作用付き恒等関数」です。`x => debug => y`
の形でパイプラインをそのまま継続できます。正式バックエンドすべてで標準
出力に書きます。

**Example**:

```taida
scores <= @[95, 82, 78, 91]
scores => Filter[_, _ x: Int = x > 80]() => debug => Map[_, _ x: Int = x * 2]() => result
// debug を挟むとフィルタ後の値が表示される
```

値の型情報を取り出したい場合は、関数ではなくモールド
`TypeName[value]()` を使います。詳細は本書 §7.10 を参照してください。

---

## 5. 整数列生成

### 5.1 `range`

> `start` から `end - 1` までの整数リストを生成する。

```taida fragment
range start: Int  end: Int => :@[Int]
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `start` | `Int` | 開始値 (生成リストに含まれる)。 |
| `end` | `Int` | 終端値 (生成リストには **含まれない**)。 |

**Returns**: `:@[Int]` — `start <= i < end` を満たす整数のリスト。
`start >= end` の場合は空リスト `@[]`。

**Example**:

```taida
range(0, 5)            // @[0, 1, 2, 3, 4]
range(5, 0)            // @[]
```

---

## 6. プロセス制御

### 6.1 `exit`

> プロセスを指定の exit code で終了する。

```taida fragment
exit code: Int => :Int
```

**Parameters**:

| Name | Type | Description |
|------|------|-------------|
| `code` | `Int` | プロセス終了コード。デフォルト値 `0` は正常終了。慣例的に異常終了は `1` 以上を使う。 |

**Returns**: `:Int` — 型注釈上は `code` と同値を返す `:Int`。

実際にはプロセスがここで終了するため、呼び出し以降のコードには制御が到達しません。Taida は `@()` / `:Unit` を「値の不在」型として認めないため (`[E1520]` 参照)、終了しない関数であっても戻り型として意味のある具体型を宣言します。

`result <= exit(1)` のような戻り値束縛は構文上は許容されますが、`result` を参照するコードには制御が到達しません。「終了することしかしない関数」を表す専用の戻り型は、現バージョンでは導入していません。

**AI-Context**:
`exit(code)` の主な用途は exit code を明示したい場合 (`exit(0)` の意図的
な正常終了 / `exit(N)` の意味付き異常終了) です。エラー経路で code を
選ばないなら、ゴリラリテラル `><` を使ってください
([`docs/guide/07_control_flow.md`](../guide/07_control_flow.md) 参照)。
`><` は `exit(1)` 固定の即時終了で、`exit(...)` よりも書きやすい正規
ルートです。

**Example**:

```taida fragment
| configValid |> startServer()
| _           |> stderr("config 不正") => exit(2)
```

「致命的エラーで終了したい」だけのケースは `exit(1)` ではなく `><` を
使う方が Taida 流です:

```taida fragment
| has_error |> stderr("致命的エラー") => ><
| _         |> stdout("正常終了")
```

---

## 7. インポート不要モールド一覧

プレリュード関数と同じく、以下のモールドはインポート不要で常に利用
できます。文字列・数値・リスト操作・条件分岐・型比較・型変換・演算
など、ほとんどのモールドはインポート不要です。モールドの解剖と概念は
[`docs/guide/05_mold.md`](../guide/05_mold.md) を参照してください。

### 7.1 数値拡張モールド

| モールド | 戻り値 | 説明 |
|---------|--------|------|
| `BitAnd[a, b]()` / `BitOr[a, b]()` / `BitXor[a, b]()` / `BitNot[x]()` | `Int` | ビット演算。 |
| `ShiftL[x, n]()` / `ShiftR[x, n]()` / `ShiftRU[x, n]()` | `Lax[Int]` | シフト (`n` は `0..63` で成功)。 |
| `ToRadix[int, base]()` | `Lax[Str]` | `base` (`2..36`) への基数変換。 |
| `Int[str, base]()` | `Lax[Int]` | 指定基数の文字列を整数化。 |

### 7.2 バイト列・Unicode モールド

| モールド | 戻り値 | 説明 |
|---------|--------|------|
| `UInt8[x]()` | `Lax[Int]` | `0..255` 制約付き変換。 |
| `Bytes[x](fill <= n)` | `Lax[Bytes]` | `Int` / `Str` / `@[Int]` / `Bytes` から Bytes へ変換。 |
| `ByteLength[x]()` | `Int` | UTF-8 byte 長または Bytes 長。 |
| `ByteAt[x, idx]()` | `Lax[Int]` | byte 位置の値を取得。 |
| `ByteSet[bytes, idx, value]()` | `Lax[Bytes]` | 指定位置更新 (不変)。 |
| `ByteSlice[x, start, end]()` | `Str` | UTF-8 byte 範囲を `Str` として取り出す。 |
| `BytesToList[bytes]()` | `@[Int]` | Bytes を整数リストへ変換。 |
| `BytesCursor[bytes](offset <= n)` | `@(bytes: Bytes, offset: Int, length: Int)` | Bytes の順次読み取りカーソルを生成。 |
| `BytesCursorRemaining[cursor]()` | `Int` | 残り読み取り可能バイト数。 |
| `BytesCursorTake[cursor, size]()` | `Lax[@(value: Bytes, cursor: @(...))]` | `size` バイト読み取り + カーソル前進。 |
| `BytesCursorU8[cursor]()` | `Lax[@(value: Int, cursor: @(...))]` | 1 バイト読み取り + カーソル前進。 |
| `U16BE[x]()` / `U16LE[x]()` | `Lax[Bytes]` | 16bit 符号なし整数を endian 指定で 2 バイトへパック。 |
| `U32BE[x]()` / `U32LE[x]()` | `Lax[Bytes]` | 32bit 符号なし整数を endian 指定で 4 バイトへパック。 |
| `U16BEDecode[bytes]()` / `U16LEDecode[bytes]()` | `Lax[Int]` | 2 バイトを endian 指定で 16bit 整数へデコード。 |
| `U32BEDecode[bytes]()` / `U32LEDecode[bytes]()` | `Lax[Int]` | 4 バイトを endian 指定で 32bit 整数へデコード。 |
| `Char[x]()` | `Lax[Str]` | 1 文字への変換。 |
| `CodePoint[str]()` | `Lax[Int]` | 1 文字のコードポイント取得。 |
| `Utf8Encode[str]()` | `Lax[Bytes]` | UTF-8 エンコード。 |
| `Utf8Decode[bytes]()` | `Lax[Str]` | UTF-8 デコード (不正列は failure)。 |

### 7.3 モールド型コンストラクタ

| モールド | 戻り値 | 説明 |
|---------|--------|------|
| `Lax[value]()` | `Lax[T]` | 失敗可能値を必ず値へ落とす安全モールド。値省略 / 失敗の表現はこの一手に集約する。 |
| `Optional[value]()` | `Lax[T]` | 廃止された旧記法。呼び出すと `Optional has been removed. Use Lax[value]() instead.` という実行時エラーが返るため、新規コードは `Lax[value]()` を使う。 |
| `Result[value, pred](throw <= error)` | `Result[T, _]` | 述語付き Result。`>=>` で述語評価、真なら値、偽なら throw。 |
| `Async[value]()` | `Async[T]` | 即時 fulfilled の非同期値。 |
| `AsyncReject[error]()` | `Async[T]` | 即時 rejected の非同期値。 |
| `AsyncTask[_ = expr]()` | `AsyncTask[T]` | CPU 計算を遅延タスクとして保持。`T` は `expr` の型。 |
| `Gorillax[value]()` | `Gorillax[T]` | 覚悟のモールド型。unmold 失敗時にゴリラ (即時終了) が発動。 |
| `RelaxedGorillax[value]()` | `RelaxedGorillax[T]` | `|==` で捕捉可能な Gorillax 形状。 |
| `Stream[value]()` | `Stream[T]` | 逐次値を表す stream wrapper。 |
| `StreamFrom[list]()` | `Stream[T]` | リストから stream を生成。 |
| `Molten[]()` | `Molten` | 外部由来の不透明値。通常は境界 API が生成する。 |
| `Moltenize[value]()` | `Moltenized[T]` | 値を不透明な封印キャリアに包む。表示・シリアライズ・直接 unmold・等値比較は型レベルで遮断される。 |
| `MoltenizeSecret[value]()` | `Secret[T]` | secret / credential 用の封印キャリア。`Moltenized[T]` と同じ遮断に加え、用途をシグネチャで明示する。 |
| `Redact[secret]()` | `Str` | 封印キャリアを固定マスク `"***"` の文字列へ落とす。内部値は読み出さない唯一の表示手段。 |
| `MoltenizeSecretFromEnv[name]()` | `Lax[Secret[Str]]` | 環境変数を封印キャリアへ直接読み込む（同期）。値は境界で封印され、平文 `Str` を経由しない。 |
| `MoltenizeSecretFromInput[prompt]()` | `Async[Lax[Secret[Str]]]` | 標準入力 1 行を封印キャリアへ読み込む。 |
| `MoltenizeSecretFromFile[path]()` | `Async[Lax[Secret[Bytes]]]` | ファイルのバイト列を封印キャリアへ読み込む。 |
| `HmacSha256[secret, message]()` | `Str` | 封印された秘密鍵で HMAC-SHA256 を計算する secret-aware consumer。秘密を平文へ revealせず鍵として使う。戻り値の MAC は公開値。 |
| `ConstantTimeEq[secret, candidate]()` | `Bool` | 封印された秘密と候補を定数時間で比較する。`==`（`[E1536]`）の代わりに秘密の照合に使う。 |
| `Reveal[secret, consumer]()` | `R` | escape hatch。`consumer: T => R` を revealed 平文に適用し `R` を返す。封印を弱めるため上の consumer を優先する（interpreter / JS のみ）。 |
| `Stub[value]()` | `T` | 「ここはまだ仮の値」と印を付けた値を返す。 |
| `TODO[]()` | `T` | 未実装の印として置く値。リリース版のビルドでは残存を拒否できる。 |
| `Cage[subject, runner]()` | `Gorillax[T]` / `Async[T]` | `Molten` を扱う境界。同期 runner は `Gorillax[T]`、Promise-returning JS runner (`JSCallAsync`) は `Async[T]` で受ける。 |
| `CageRilla[Branch, Out]()` | `Pack` | `Cage` で `runner` 位置に置く実行記述の親型。直接呼び出さない。 |
| `JSRilla[Out]()` | `Pack` | JS 連携の実行記述。`JSGet` / `JSCall` / `JSCallAsync` 等が返す。直接呼び出さない。 |
| `FileRilla[Out]()` | `Pack` | ファイル branch の実行記述の抽象型。具体 runner が公開されている API からのみ使う。 |
| `BuildRilla[Out]()` | `Pack` | Build branch の実行記述の抽象型。具体 runner が公開されている API からのみ使う。 |
| `JSON[raw, Schema]()` | `Lax[T]` | JSON を schema 指定で Taida 値へ変換。 |

> `CageRilla[Branch, Out]` および `JSRilla[Out]` / `FileRilla[Out]` /
> `BuildRilla[Out]` は **直接呼び出さない型** です。`Cage[subject,
> runner]()` の型規則と、`runner` 位置に置く実行記述 (`JSGet` /
> `JSCall` / `JSCallAsync` 等) の戻り型としてのみ公開面に現れます。
> File / Build branch は、対応する具体 runner を公開している API の
> リファレンスで説明されている場合にだけ使います。JS 実行記述の詳細は
> [`docs/api/js.md`](js.md) を参照してください。

#### 封印キャリア (Moltenized / Secret)

`Moltenize[v]()` / `MoltenizeSecret[v]()` は値を `Molten` の不透明性を継承する
封印キャリアに包みます。secret / credential を平文の `Str` として持ち回らず、
観測される経路を型レベルで遮断するための型です。包んだ値の観測は
`Redact[secret]()` (固定マスク `"***"`) を介してのみ行えます。

```taida
secret <= MoltenizeSecret["api-key"]()
stdout(Redact[secret]())
```

型チェッカーは封印された値が漏れ口に届くことをコンパイル時に拒否します:

- 表示 (`stdout` / `stderr` / `debug`、`.toString()` / `.toStr()`、`Str[secret]()`、文字列補間 `` `${secret}` ``) — `[E1533]`
- シリアライズ (`jsonEncode` / `jsonPretty`、`@(...)` / `@[...]` 構造内の封印値を含む) — `[E1534]`
- 直接 unmold (`>=>` / `<=<`) — `[E1535]`
- 二項演算・等値・メンバシップ (`+` 連結・`==` / `!=`・`.contains()` / `.indexOf()` のオラクル) — `[E1536]`

万一コンパイル時検査を省いた場合 (`--no-check`) でも、全バックエンド (interpreter /
JS / native / wasm) のランタイムは標準 sink を fail-closed で処理し、封印値の代わりに
policy ラベル (`<Secret>` / `<Moltenized>`) を返します。診断コードの詳細は
[`docs/reference/diagnostic_codes.md`](../reference/diagnostic_codes.md)
を参照してください。

**秘密を観測せずに使う (secret-aware consumer):** 封印された秘密は、平文へ戻さずに
そのまま消費できます。

```taida fragment
unwrapped <= MoltenizeSecretFromEnv["API_KEY"]()
sig <= HmacSha256[unwrapped, requestBody]()
stdout(sig)
ok <= ConstantTimeEq[unwrapped, providedToken]()
```

- `HmacSha256[secret, message]()` — 封印鍵で署名を計算。戻り値 (MAC) は公開値で、
  封印キャリア型を離れます。
- `ConstantTimeEq[secret, candidate]()` — 秘密と候補を定数時間で比較。秘密の照合は
  `==`（`[E1536]` で拒否）ではなく必ずこれを使います。

これらの第1引数は `Secret`/`Moltenized` が `Str`/`Bytes` を包んだ封印キャリア、第2引数は
非秘密の `Str`/`Bytes` であることを型チェッカーが強制します。非封印の鍵や封印された
第2引数は `[E1506]` でコンパイル時に拒否されます (全バックエンド共通)。

`Reveal[secret, consumer]()` は **escape hatch** です。`consumer: T => R` を revealed
平文に適用して `R` を返します。封印を弱める（平文が consumer のスコープに入る）ため、
上の secret-aware consumer で済む場合はそちらを使ってください。`Reveal` は interpreter と
JS バックエンドでのみ利用でき、native / WASM では capability error になります。

`Reveal` による de-seal 点は `taida way verify --check secret-flow` で監査できます。秘密が
平文として consumer に入る箇所を全て列挙するので、consumer の戻り値が平文を再漏洩しない
ことをレビューで確認してください。

**脅威モデルとメモリ保証 (Level):** 封印キャリアの第一目的は、秘密が **誤って** 平文の
`Str` として表示・シリアライズ・比較される経路を塞ぐことです。**プロセスの全メモリを
読める攻撃者** への完全防御は脅威モデルの対象外です。メモリ上の保証はバックエンドごとに
異なります。

| バックエンド | メモリ保証 | 内容 |
|------------|-----------|------|
| Interpreter | Level 1 (ZeroizedBuffer) | 封印値は破棄時に zeroize される。生存中のメモリは脅威モデル外 |
| Native / WASM / JS | Level 0 (RedactedOpaque) | sink 遮断のみ。封印値はメモリ上に平文で保持され、消費時にコピーが生じうる |

`HmacSha256` / `ConstantTimeEq` を使った時点で、各バックエンドの暗号プリミティブは内部で
鍵バッファを扱います（interpreter 以外では平文コピーを materialize します）。秘密を扱う
本番ワークロードでは、メモリ保証が必要なバックエンド (interpreter) を選んでください。

### 7.4 文字列モールド

文字列の変換・加工を行うモールドです。状態チェック (`length()` /
`contains()` / `startsWith()` 等) はメソッドとして本書 §8.1 に集約しています。

| モールド | `[]` 必須 | `()` オプション | 戻り値 | 説明 |
|---------|----------|----------------|--------|------|
| `Upper[str]()` | str | — | `Str` | 大文字変換 |
| `Lower[str]()` | str | — | `Str` | 小文字変換 |
| `Trim[str]()` | str | `start`, `end` | `Str` | 空白除去 (`start` / `end` でデフォルト `true` を `false` に) |
| `Split[str, delim]()` | str, delim | — | `@[Str]` | 区切り文字で分割 |
| `Replace[str, old, new]()` | str, old, new | `all` | `Str` | 置換 (`all <= true` で全置換) |
| `ReplaceAll[str, old, new]()` | str, old, new | — | `Str` | 全一致を置換 |
| `Slice[str]()` | str | `start`, `end` | `Str` | 範囲抽出 |
| `Chars[str]()` | str | — | `@[Str]` | 文字単位のリストへ分解 |
| `CharAt[str, idx]()` | str, idx | — | `Lax[Str]` | 指定位置の文字 (範囲外で failure) |
| `Contains[str, needle]()` | str, needle | — | `Bool` | 部分文字列を含むか |
| `IndexOf[str, needle]()` | str, needle | — | `Int` | 最初の出現位置 (-1 で見つからず) |
| `LastIndexOf[str, needle]()` | str, needle | — | `Int` | 最後の出現位置 (-1 で見つからず) |
| `Repeat[str, n]()` | str, n | — | `Str` | 文字列の繰り返し |
| `Reverse[str]()` | str | — | `Str` | 逆順 |
| `Pad[str, len]()` | str, len | `side`, `char` | `Str` | パディング (`side <= "start" / "end"`) |
| `PadLeft[str, len, char]()` | str, len, char | — | `Str` | 左パディング |
| `PadRight[str, len, char]()` | str, len, char | — | `Str` | 右パディング |
| `StringRepeatJoin[str, n, sep]()` | str, n, sep | — | `Str` | 繰り返し文字列を separator で結合 |

```taida fragment
Upper["hello"]()                              // "HELLO"
Trim["  hello  "](end <= false)               // "hello  " (先頭のみ)
Replace["hello world", "o", "0"](all <= true) // "hell0 w0rld"
Pad["42", 5](side <= "start", char <= "0")    // "00042"
```

### 7.5 数値モールド

数値の変換・加工を行うモールドです。状態チェック (`isNaN()` /
`isZero()` / `isPositive()` 等) はメソッドとして提供されます。
ビット演算・シフト・基数変換は §7.1、バイト列・Unicode 変換は §7.2 を
参照してください。

| モールド | `[]` 必須 | `()` オプション | 戻り値 | 説明 |
|---------|----------|----------------|--------|------|
| `ToFixed[num, digits]()` | num, digits | — | `Str` | 小数点固定文字列 |
| `Sqrt[num]()` | num | — | `Float` | 平方根 |
| `Pow[x, y]()` | x, y | — | `Float` | 累乗 |
| `Log[x]()` | x | — | `Float` | 自然対数 |
| `Log[x, base]()` | x, base | — | `Float` | 任意底の対数 |
| `Exp[x]()` | x | — | `Float` | 指数関数 |
| `Ln[x]()` | x | — | `Float` | 自然対数 |
| `Log2[x]()` | x | — | `Float` | 2 底対数 |
| `Log10[x]()` | x | — | `Float` | 10 底対数 |
| `Sin[x]()` | x | — | `Float` | 正弦 |
| `Cos[x]()` | x | — | `Float` | 余弦 |
| `Tan[x]()` | x | — | `Float` | 正接 |
| `Asin[x]()` | x | — | `Float` | 逆正弦 |
| `Acos[x]()` | x | — | `Float` | 逆余弦 |
| `Atan[x]()` | x | — | `Float` | 逆正接 |
| `Atan2[y, x]()` | y, x | — | `Float` | 2 引数逆正接 |
| `Sinh[x]()` | x | — | `Float` | 双曲線正弦 |
| `Cosh[x]()` | x | — | `Float` | 双曲線余弦 |
| `Tanh[x]()` | x | — | `Float` | 双曲線正接 |
| `Abs[num]()` | num | — | `Num` | 絶対値 |
| `Floor[num]()` | num | — | `Int` | 切り捨て |
| `Ceil[num]()` | num | — | `Int` | 切り上げ |
| `Round[num]()` | num | — | `Int` | 四捨五入 |
| `Truncate[num]()` | num | — | `Int` | 0 方向切り捨て |
| `Clamp[num, min, max]()` | num, min, max | — | `Num` | 範囲制限 |

```taida fragment
ToFixed[3.14159, 2]()    // "3.14"
Abs[-5]()                 // 5
Round[3.5]()              // 4
Clamp[15, 0, 10]()        // 10
```

### 7.6 リストモールド

リスト `@[T]` の操作を行うモールドです。状態チェック (`length()` /
`isEmpty()` / `contains()` / `any()` / `all()` / `none()` 等) と安全
アクセス (`get()` / `first()` / `last()` / `max()` / `min()`) はメソッド
として提供されます。

| モールド | `[]` 必須 | `()` オプション | 戻り値 | 説明 |
|---------|----------|----------------|--------|------|
| `Map[list, fn]()` | list, fn | — | `@[U]` | 各要素を変換 |
| `Filter[list, fn]()` | list, fn | — | `@[T]` | 条件で絞り込み |
| `Fold[list, init, fn]()` | list, init, fn | — | `A` | 左畳み込み |
| `Foldr[list, init, fn]()` | list, init, fn | — | `A` | 右畳み込み |
| `Take[list, n]()` | list, n | — | `@[T]` | 先頭 n 個取得 |
| `Drop[list, n]()` | list, n | — | `@[T]` | 先頭 n 個スキップ |
| `TakeWhile[list, fn]()` | list, fn | — | `@[T]` | 条件満たす間取得 |
| `DropWhile[list, fn]()` | list, fn | — | `@[T]` | 条件満たす間スキップ |
| `Append[list, val]()` | list, val | — | `@[T]` | 末尾追加 |
| `Prepend[list, val]()` | list, val | — | `@[T]` | 先頭追加 |
| `Concat[list, other]()` | list, other | — | `@[T]` | リスト結合 |
| `Sort[list]()` | list | `reverse`, `by` | `@[T]` | ソート (`by` はキー抽出関数) |
| `Reverse[list]()` | list | — | `@[T]` | 逆順 |
| `Unique[list]()` | list | `by` | `@[T]` | 重複除去 |
| `Flatten[list]()` | list | — | `@[U]` | 1 段階フラット化 |
| `Join[list, sep]()` | list, sep | — | `Str` | 文字列結合 |
| `Sum[list]()` | list | — | `Num` | 数値合計 |
| `Min[list]()` | list | — | `T` | 最小値 |
| `Max[list]()` | list | — | `T` | 最大値 |
| `Find[list, fn]()` | list, fn | — | `Lax[T]` | 条件を満たす最初の要素 |
| `FindIndex[list, fn]()` | list, fn | — | `Int` | 条件を満たす最初の位置 (-1 で見つからず) |
| `FindIndexLax[list, fn]()` | list, fn | — | `Lax[Int]` | 条件を満たす最初の位置 |
| `Count[list, fn]()` | list, fn | — | `Int` | 条件を満たす要素数 |
| `Length[list]()` | list | — | `Int` | 要素数 |
| `Reduce[list, init, fn]()` | list, init, fn | — | `A` | 左畳み込み |
| `Zip[list, other]()` | list, other | — | `@[BuchiPack]` | ペア化 |
| `Enumerate[list]()` | list | — | `@[BuchiPack]` | インデックス付与 |

```taida fragment
Map[@[1, 2, 3], _ x = x * 2]() >=> doubled        // @[2, 4, 6]
Filter[@[85, 92, 78], _ x = x >= 90]() >=> high  // @[92]
Fold[@[1, 2, 3, 4, 5], 0, _ acc x = acc + x]() >=> total  // 15
Sort[@[3, 1, 4, 1, 5]](reverse <= true) >=> desc  // @[5, 4, 3, 1, 1]
```

### 7.7 演算モールド

`/` と `%` 演算子は Taida にはありません。除算と剰余はモールドで行い、
結果は `Lax` で返ります。

| モールド | `[]` 必須 | 戻り値 | 説明 |
|---------|----------|--------|------|
| `Div[x, y]()` | x, y | `Lax[Num]` | 除算 (ゼロ除算で `has_value=false`) |
| `Mod[x, y]()` | x, y | `Lax[Num]` | 剰余 (ゼロ除算で `has_value=false`) |

```taida fragment
Div[10, 3]() >=> q   // 3
Div[10, 0]() >=> q   // 0 (ゼロ除算: デフォルト値)
Div[10, 0]().has_value   // false
```

### 7.8 条件モールド

| モールド | `[]` 必須 | 戻り値 | 説明 |
|---------|----------|--------|------|
| `If[cond, then, else]()` | cond, then, else | `T` | 2 分岐の条件式 (短絡評価) |

```taida fragment
result <= If[x > 0, "positive", "negative"]()

// パイプラインで _ を複数回参照 (clamp パターン)
150 => If[_ > 100, 100, _]() => clamped   // 100
```

`If` は **2 分岐向き** です。3 分岐以上は `| cond |> value` 構文を使用
してください。詳細は
[`docs/guide/07_control_flow.md`](../guide/07_control_flow.md) を参照
してください。

### 7.9 型変換モールド

値の型変換はモールドで行います。結果は `Lax` で返り、変換失敗時は
`has_value=false` でデフォルト値にフォールバックします。

| モールド | `[]` 必須 | 戻り値 | 説明 |
|---------|----------|--------|------|
| `Int[x]()` | x | `Lax[Int]` | 整数化 (`Int["123"]()` → 123、`Int["abc"]()` → 0) |
| `Int[str, base]()` | str, base | `Lax[Int]` | 指定基数の文字列を整数化 (§7.1 と同一) |
| `Float[x]()` | x | `Lax[Float]` | 浮動小数化 (`Float["3.14"]()` → 3.14) |
| `Str[x]()` | x | `Lax[Str]` | 文字列化 (`Str[42]()` → "42") |
| `Bool[x]()` | x | `Lax[Bool]` | 真偽値化 (`Bool[1]()` → true、`Bool[0]()` → false) |
| `Ordinal[e]()` | e | `Int` | Enum を宣言順の Int (ordinal) に変換。Enum 以外を渡すと実行時エラーになる |

```taida fragment
Int["ff", 16]() >=> hex   // 255
Float["abc"]() >=> v      // 0.0 (失敗: デフォルト値)

Enum => Color = :Red :Green :Blue
Ordinal[Color:Green()]()  // 1
```

`Ordinal[]` は Enum → Int の唯一の正規経路です。`.toString()` を `Int[]`
で parse するパターンは fragile なので使わないでください。逆方向
(`Int → Enum`) は別 track で検討中です。

### 7.10 型比較モールド

実行時の型チェックと型継承関係チェックをモールドで行います。

| モールド | `[]` 必須 | 戻り値 | 説明 |
|---------|----------|--------|------|
| `TypeIs[value, :TypeName]()` | value, :TypeName | `Bool` | 値の実行時型と一致するかを返す |
| `TypeIs[value, EnumName:Variant]()` | value, EnumName:Variant | `Bool` | Enum variant 一致判定 |
| `TypeExtends[:TypeA, :TypeB]()` | :TypeA, :TypeB | `Bool` | TypeA が TypeB と同じか TypeB のサブタイプか |
| `TypeName[value]()` | value | `Str` | 値の型情報 (継承位置 / variant 名 / プリミティブ型名) を返す |

```taida fragment
TypeIs[42, :Int]()         // true
TypeIs[42, :Num]()         // true (Int は Num の一種)

Enum => Status = :Ok :Fail
x <= Status:Ok()
TypeIs[x, Status:Ok]()     // true

TypeExtends[:Int, :Num]()  // true

TypeName[42]()             // "Int"
TypeName[Status:Ok()]()    // "Ok"
```

対応する型リテラルの範囲: プリミティブ (`:Int` / `:Float` / `:Num` /
`:Bool` / `:Str` / `:Bytes` / `:Error`)、ユーザー定義型 (`:TypeName`)、
Enum variant (`EnumName:Variant`、TypeIs のみ)。inline BuchiPack 型
リテラル / 関数型リテラル / 汎用 generic literal は対応外です。

`__type` フィールドへの直接アクセス (`err.__type` 等) は `[E1960]` で
reject されるため、継承位置や variant 名を読みたい場合は必ず
`TypeName[x]()` を使ってください。

### 7.11 非同期合成モールド

`Async[T]` の合成は、待ち方の詳細を利用者に露出しないためにモールドで
表します。I/O 由来の非同期値も、`AsyncTask` 由来の CPU タスクも、
`>=>` / `<=<` で同じように待ちます。

| モールド | `[]` 必須 | 戻り値 | 説明 |
|---------|----------|--------|------|
| `Cancel[async]()` | async | `Async[T]` | 非同期処理の cancellation を要求 |
| `All[list]()` | list | `Async[@[T]]` | 全 async の完了を待つ |
| `Race[list]()` | list | `Async[T]` | 最初に完了した async を返す |
| `Timeout[async, ms]()` | async, ms | `Async[T]` | 指定時間で timeout |
| `Par[jobs]()` | jobs | `Async[@[T]]` | `@[AsyncTask[T]]` を実行し、入力順の結果リストを返す |
| `ParMap[list, fn]()` | list, fn | `Async[@[U]]` | `list` の各要素に `fn` を適用し、入力順の結果リストを返す |

`Par` / `ParMap` の公開契約は、結果順、失敗伝播、待機挙動がバックエンド間で
一致することです。実行戦略は worker dispatch でも deterministic sequential
fallback でもよく、利用者が依存してよいのは `Async` として観測できる結果だけ
です。

`AsyncTask` 本体と `ParMap` の mapper は純粋な計算に限定されます。I/O、
ネットワーク、プロセス操作、addon / host interop、ネストした非同期・並列構造、
転送できない値の捕捉は checker が `E1620`〜`E1626` の診断で reject します。

---

## 8. ビルトイン型メソッド

メソッドは **状態チェック** (内部状態を `Bool` / `Int` で返す)、
**安全アクセス** (`Lax` で返す)、**モナディック操作** (`map` / `flatMap` /
`mapError`)、**`.toString()`** に限定されています。値の変換・加工はモールド
で行います (§7 を参照)。

### 8.0 `.toString()` — 全型共通

すべての値は `.toString() => :Str` を呼び出せます。正式バックエンドで同一の
文字列を返します。

```taida
42.toString()             // "42"
3.14.toString()           // "3.14"
true.toString()           // "true"
"hello".toString()        // "hello"
@[1, 2, 3].toString()     // "@[1, 2, 3]"
@(a <= 1, b <= 2).toString()  // "@(a <= 1, b <= 2)"
```

引数なしで呼び出すこと。`n.toString(16)` のように base を渡そうとすると、
チェッカーが `[E1508]` で拒否します。基数指定で整数を文字列化したい場合は
`ToRadix[n, base]()` モールドを使い、`.getOrDefault("")` で unwrap します。

### 8.1 Str メソッド

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `length()` | `=> :Int` | 文字数 |
| `contains(sub)` | `Str => :Bool` | 部分一致 |
| `startsWith(prefix)` | `Str => :Bool` | 先頭一致 |
| `endsWith(suffix)` | `Str => :Bool` | 末尾一致 |
| `indexOfLax(sub)` | `Str => :Lax[Int]` | 最初の出現位置 (推奨) |
| `lastIndexOfLax(sub)` | `Str => :Lax[Int]` | 最後の出現位置 (推奨) |
| `get(idx)` | `Int => :Lax[Str]` | 指定位置の文字 (範囲外で failure) |
| `replace(target, repl)` | `Str, Str => :Str` または `Regex, Str => :Str` | 最初の一致を置換 |
| `replaceAll(target, repl)` | `Str, Str => :Str` または `Regex, Str => :Str` | 全置換 |
| `split(sep)` | `Str => :@[Str]` または `Regex => :@[Str]` | 分割 (空 sep で文字単位) |
| `match(pattern)` | `Regex => :RegexMatch` | 最初の一致を `@(has_value, full, groups, start)` で返す |
| `searchLax(pattern)` | `Regex => :Lax[Int]` | 一致位置を `Lax[Int]` で返す (推奨) |

`indexOf` / `lastIndexOf` / `search` は `-1` を返す旧 API で非推奨です。
`*Lax` 版に移行してください。

#### Regex コンストラクタ

```taida fragment
Regex pattern: Str  flags: Str => :Regex
```

サポートされるフラグ: `i` (大文字小文字無視)、`m` (複数行モード)、`s` (dotall
— Interpreter のみ。Native の POSIX ERE はサポート外)。

サポートされるエスケープ: `\d` / `\D` / `\w` / `\W` / `\s` / `\S` / `\xHH` /
`\x{HH…}` / `\uHHHH` / `\u{HH…}` / `\\` は正式バックエンド対応、`\b` / `\B` は
Interpreter のみ対応。Native の POSIX ERE は単語境界の概念を持ちません。

不正なフラグや不正なパターンは正式バックエンドすべてで構築時に `:Error`
(`ValueError`) が投げられます。JavaScript の `$&` / `$1` 等の置換メタ
構文は無効化されており、置換文字列はリテラルとして挿入されます。

### 8.2 Num (Int / Float) メソッド

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `isNaN()` | `=> :Bool` | NaN 判定 (`Div` / `Mod` が `Lax` を返すため、通常生成されない。外部データの検査用) |
| `isInfinite()` | `=> :Bool` | 無限大判定 |
| `isFinite()` | `=> :Bool` | 有限数判定 |
| `isPositive()` | `=> :Bool` | 正数判定 (0 は false) |
| `isNegative()` | `=> :Bool` | 負数判定 |
| `isZero()` | `=> :Bool` | ゼロ判定 |

数値の変換 (`Abs` / `Floor` / `Ceil` / `Round` / `Truncate` / `Clamp` /
`ToFixed`) はモールドで行います (§7.5)。

### 8.3 Bool メソッド

`toString() => :Str` のみ。型変換は `Int[true]()` / `Int[false]()` で行います
(§7.9)。

### 8.4 Bytes メソッド

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `length()` | `=> :Int` | バイト数 |
| `get(idx)` | `Int => :Lax[Int]` | 指定位置のバイト値 |
| `toString()` | `=> :Str` | `"Bytes[@[...]]"` 形式 |

操作 (`ByteSet` / `Slice` / `Concat` / `BytesToList` / `Utf8Decode`) はモールド
で行います (§7.2)。

### 8.5 List メソッド

#### 状態チェック

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `length()` | `=> :Int` | 要素数 |
| `isEmpty()` | `=> :Bool` | 空判定 |
| `contains(item)` | `T => :Bool` | 要素含有 |
| `indexOfLax(item)` | `T => :Lax[Int]` | 位置 (推奨) |
| `lastIndexOfLax(item)` | `T => :Lax[Int]` | 最終位置 (推奨) |

#### 安全アクセス

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `first()` | `=> :Lax[T]` | 最初の要素 |
| `last()` | `=> :Lax[T]` | 最後の要素 |
| `get(idx)` | `Int => :Lax[T]` | 指定位置の要素 |
| `max()` | `=> :Lax[T]` | 最大値 |
| `min()` | `=> :Lax[T]` | 最小値 |

#### 述語

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `any(pred)` | `(T => :Bool) => :Bool` | いずれかが満たす |
| `all(pred)` | `(T => :Bool) => :Bool` | 全てが満たす |
| `none(pred)` | `(T => :Bool) => :Bool` | 全てが満たさない |

#### 畳み込み

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `fold(init, fn)` | `(Acc, (Acc, T) => :Acc) => :Acc` | 初期値から左畳み込み (`Fold[]` モールドのメソッド版) |
| `reduce(init, fn)` | `(Acc, (Acc, T) => :Acc) => :Acc` | `fold` の別名 (同一シグネチャ) |

その他の操作 (`Map` / `Filter` / `Sort` 等) はモールドで行います (§7.6)。

### 8.6 Lax メソッド

`Lax[T]` 型に対して使用します。

| メソッド / フィールド | シグネチャ | 説明 |
|---------------------|-----------|------|
| `has_value` | フィールド (`Bool`) | 値を持つかどうか |
| `isEmpty()` | `=> :Bool` | `!has_value` |
| `getOrDefault(default)` | `T => :T` | 値があればそれ、なければ default |
| `unmold()` | `=> :T` | 値を取り出す (失敗時は型 T のデフォルト値) |
| `map(fn)` | `(T => :U) => :Lax[U]` | モナディック変換 |
| `flatMap(fn)` | `(T => :Lax[U]) => :Lax[U]` | モナディック連鎖 |
| `errorInfo()` | `=> :Lax[ErrorInfo]` | 失敗詳細を取り出す |
| `toString()` | `=> :Str` | `"Lax(3)"` / `"Lax(default: 0)"` 等 |

#### `map` / `flatMap` の引数型ピンの効力範囲

`map` / `flatMap` は受け取る関数のシグネチャ全体 (引数型と戻り値型) を
チェッカーが固定し、`fn` の引数型が `T` と食い違えば `[E1508]` で reject
されます。ただしこのピンは **受け側の `T` が具体型として確定しているとき**
にのみ機能します。クロスモジュール import 等で `T` が未解決のまま call site
に到達した場合、未解決側がサブタイプ規則のワイルドカードとして振る舞い、
本来 `[E1508]` で reject されるはずの関数も checker の reject 漏れとして
通過してしまいます。

確定させるには以下のいずれか:

- 受け側に **引数の型注釈付き local 関数** を定義して間に挟む (`unwrap v: Lax[Int] = v => :Lax[Int]`)
- import を経由せず、`Lax[42]()` のような **値ベースのコンストラクタ** で受け側に具体型を直接組み立てる
- 中間に `getOrDefault(...)` を挟んで inner を取り出してから処理する

同じ caveat は `Result` / `Async` の `map` / `flatMap` / `mapError` にも
当てはまります。

### 8.7 Gorillax / RelaxedGorillax メソッド

| メソッド / フィールド | レシーバ | シグネチャ | 説明 |
|---------------------|---------|-----------|------|
| `has_value` | 両方 | フィールド (`Bool`) | 値を持つか |
| `isEmpty()` | 両方 | `=> :Bool` | `!has_value` |
| `relax()` | `Gorillax[T]` | `=> :RelaxedGorillax[T]` | `\|==` で捕捉可能な版に変換 |
| `errorInfo()` | 両方 | `=> :Lax[ErrorInfo]` | 失敗詳細 |
| `toString()` | 両方 | `=> :Str` | 文字列表現 |

`Gorillax` の unmold 失敗時はゴリラ (プログラム即終了) が発動します。
`relax()` 後は `RelaxedGorillaEscaped` エラーの throw に変わり、`|==` で
catch できます。

### 8.8 Result メソッド

`Mold[T] => Result[T, P <= :T => :Bool] = @(throw: Error)` として定義され、
第 1 型引数 `T` は成功時の値の型、第 2 型引数 `P` は内部の判定述語
(`:T => :Bool`) を経由して **throw 時の payload 型** として観測されます。

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `throw` | フィールド (`Error`) | エラー値 |
| `isSuccess()` | `=> :Bool` | 成功か (述語 P が真) |
| `isError()` | `=> :Bool` | エラーか (述語 P が偽) |
| `getOrDefault(default)` | `T => :T` | 安全な値取得 |
| `getOrThrow()` | `=> :T` | 値取得 (失敗時 throw) |
| `map(fn)` | `(T => :U) => :Result[U, _]` | モナディック変換 |
| `flatMap(fn)` | `(T => :Result[U, _]) => :Result[U, _]` | モナディック連鎖 |
| `mapError(fn)` | `(P => :Q) => :Result[T, Q]` | throw payload 変換 |
| `toString()` | `=> :Str` | 文字列表現 |

取り出しはアンモールド演算子 (`result >=> v` / `v <=< result`) で行います (成功時は値、失敗時は throw が伝播)。`errorInfo()` は `Lax` / `Gorillax` / `RelaxedGorillax` / `Error` 値のメソッドで、`Result` にはありません。

`flatMap` は受け取る関数が返す `Result` の述語型 `P` が受け側と一致する
ことを要求します。異なる述語型の `Result` を `flatMap` で混ぜようとすると
拒否されます。述語型を切り替えたいときは `mapError` を経由して明示的に
変換してください。

### 8.9 Async メソッド

| メソッド | シグネチャ | 説明 |
|---------|-----------|------|
| `isPending()` | `=> :Bool` | 実行中 |
| `isFulfilled()` | `=> :Bool` | 完了 |
| `isRejected()` | `=> :Bool` | 失敗 |
| `getOrDefault(default)` | `T => :T` | 安全な値取得 |
| `map(fn)` | `(T => :U) => :Async[U]` | モナディック変換 |
| `toString()` | `=> :Str` | 文字列表現 |
| `unmold()` | `=> :T` | アンモールド |

### 8.10 Molten / JSON はメソッド無し

`Molten` 型と `JSON` 型はメソッドを一切持ちません。Molten は外部由来の
不透明値で、直接操作はできません。

```taida
>>> npm:lodash => @(lodash)  // lodash: Molten

lodash.sum()       // エラー: Molten has no methods
lodash.toString()  // エラー: Molten has no methods
```

JS / npm 連携は `Cage[subject, JSCall[...]()]()` または
`Cage[subject, JSCallAsync[...]()]()` 経由で行います。詳細は
[`docs/api/js.md`](js.md) を参照してください。

JSON のパースは `JSON[raw, Schema]()` モールドで行います。詳細は
[`docs/guide/03_json.md`](../guide/03_json.md) を参照してください。

### 8.11 ErrorInfo シェイプ

`errorInfo()` の戻り値 `Lax[ErrorInfo]` が持つ pack のシェイプ:

```taida fragment
ErrorInfo = @(
  type: Str        // error type 名 ("HttpError" / "IoError" 等)
  message: Str     // 人間向けメッセージ
  kind: Str        // 細分カテゴリ ("timeout" / "not_found" 等。空文字なら未指定)
  code: Int        // numeric code (OS error 番号 / HTTP status / 0 = 未指定)
)
```

各フィールドは default 値を持つため `Lax[ErrorInfo]` の default は
`@(type <= "", message <= "", kind <= "", code <= 0)` です。
`getOrDefault(...)` で既定値を上書きしながら取り出すこともできます。

`__error` フィールドへの直接アクセス (`.__error.message` 等) は `[E1960]`
で reject されるため、失敗詳細を読む公式 accessor として `errorInfo()` を
使います。

---

## 9. コレクション

### 9.1 HashMap

`hashMap()` で空の HashMap を生成します。イミュータブルで、変更操作は
新しい HashMap を返します。

```taida
pilots <= hashMap().set("Misato", @(age <= 29, role <= "Operations Director")).set("Ritsuko", @(age <= 30, role <= "Chief Scientist"))
```

| メソッド | 戻り値 | 説明 |
|---------|--------|------|
| `.get(key)` | `Lax[V]` | キーに対応する値 |
| `.set(key, value)` | `HashMap[K, V]` | キーと値を追加した新しい HashMap |
| `.remove(key)` | `HashMap[K, V]` | キーを削除した新しい HashMap |
| `.has(key)` | `Bool` | キー存在判定 |
| `.keys()` | `@[K]` | キーリスト |
| `.values()` | `@[V]` | 値リスト |
| `.entries()` | `@[@(key, value)]` | キー値ペアリスト |
| `.size()` | `Int` | エントリ数 |
| `.merge(other)` | `HashMap[K, V]` | 2 つの HashMap を結合 |
| `.isEmpty()` | `Bool` | 空判定 |
| `.toString()` | `Str` | 文字列表現 |

`a.merge(b)` は **「保持してから追記」** の順で動きます。具体的には次の 2 段階です。

1. レシーバ `a` のうち `b` にキーがないエントリを、`a` の挿入順そのままで残します。
2. 続けて `b` の全エントリを、`b` の挿入順そのままで末尾に追加します。

両方に存在するキーは `b` 側の位置へ移動し、値も `b` のものになります。

`HashMap.entries()` が返すぶちパックのフィールド名は **`key` / `value`** に統一されています。`zip()` / `Zip[]()` は別仕様で `first` / `second` を使うため、`.entries()` と `zip()` のフィールド名が異なる点に注意してください。

### 9.2 Set

`setOf(list)` でリストから Set を生成します。イミュータブルで、変更操作は
新しい Set を返します。

```taida
pilot_names <= setOf(@["Misato", "Ritsuko", "Shinji"])
```

| メソッド | 戻り値 | 説明 |
|---------|--------|------|
| `.add(item)` | `Set[T]` | 要素追加 |
| `.remove(item)` | `Set[T]` | 要素削除 |
| `.has(item)` | `Bool` | 要素存在判定 |
| `.union(other)` | `Set[T]` | 和集合 |
| `.intersect(other)` | `Set[T]` | 積集合 |
| `.diff(other)` | `Set[T]` | 差集合 |
| `.toList()` | `@[T]` | リスト化 |
| `.size()` | `Int` | 要素数 |
| `.isEmpty()` | `Bool` | 空判定 |
| `.toString()` | `Str` | 文字列表現 |

---

## 10. Span ぶちパックの文字列化

`taida-lang/net` の `httpServe` / `httpParseRequestHead` が返す `@(start: Int, len: Int)` 形式のぶちパックは、元の `Bytes` をコピーせず、その範囲を指す参照として保持します。これを明示的に `Str` へ変換したいときは、次の 2 系統を使います。

```taida fragment
str_a <= strOf(req.path, req.bytes)            // 関数形式
str_b <= StrOf[req.path, req.bytes]()          // モールド形式
```

両者は同じ結果を返します。`strOf` は関数呼び出し式チェーン (`callSign(req).path` のような形) のなかで `StrOf[...]()` の括弧の二重を避けたいときに使います。

頻繁に走る経路 (ルーターでの比較など) では、文字列化せずに `SpanEquals` / `SpanStartsWith` / `SpanContains` / `SpanSlice` を使ってください。詳細は [`docs/api/net.md §4`](net.md) を参照してください。

---

## 11. バックエンド対応

プレリュード関数とビルトイン型メソッドはすべて正式バックエンド (インタプリタ /
ネイティブ / WASM 全プロファイル) で同一の挙動を返します。例外は
次のとおりです。

| 関数 / メソッド | 例外バックエンド | 補足 |
|----------------|------------------|------|
| `stdinLine` | `wasm-min` / `wasm-edge` で利用不可 | WASI 入力の TTY 抽象が必要。 |
| Regex `\b` / `\B` | Native POSIX ERE で非対応 | 単語境界が必要なら Interpreter に限定、または `(^|[^A-Za-z0-9_])` で代替。 |
| Regex `s` フラグ (dotall) | Native POSIX ERE で非対応 | Interpreter のみ。 |

詳細なバックエンドごとの差異は
[`docs/reference/wasm_profiles.md`](../reference/wasm_profiles.md) と
[`docs/reference/memory_model.md`](../reference/memory_model.md) を参照
してください。
