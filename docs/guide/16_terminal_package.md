# Terminal パッケージ

> **PHILOSOPHY.md — IV.** キモい言語だ。だが、書くのも読むのもAI。眺めるのが人間の仕事。

`taida-lang/terminal` はターミナル UI を構築するための公式アドオンです。
TTY 検出、ターミナルサイズ取得、キー / マウス入力、raw mode、画面 / カーソル
制御、ANSI スタイリング (16 / 256 / RGB)、Unicode 表示幅、仮想スクリーン
バッファ + 差分レンダラ、行エディタ、UX ウィジェットなど、61 個のシンボルを
公開します。

**位置付け**: addon (Rust `cdylib` + Taida facade)。core bundled の
`taida-lang/os` / `taida-lang/net` と異なり、`taida install` でユーザの
プロジェクトに導入します。Backend は **Native (Rust addon ABI v1) のみ**
対応 — interpreter は addon ABI 経由で cdylib に dispatch、JS / WASM は
直接サポート対象外 (graceful degrade)。

---

## 1. 概要 — addon load 手順

### インストール

```bash
taida install taida-lang/terminal
```

`taida.lock` に SHA-256 が pin され、cdylib は `.taida/deps/<pkg>/native/`
に展開されます。詳しくは [`docs/guide/13_creating_addons.md`](13_creating_addons.md)
の addon resolver 章を参照してください。

### import

必要な surface だけを `>>>` で取り込みます。bundled パッケージと同じ
構文ですが、addon は packages.tdm で依存宣言が必要です:

```taida
>>> taida-lang/terminal => @(IsTerminal, TerminalSize, ReadKey, KeyKind, Stylize, Color, ResetStyle)
```

呼び出し規則:

- **Native entries** (TTY 検出 / raw mode / I/O / レンダラ確保 / 幅計算)
  は **mold-call 構文** `Name[]()` または `Name[](args)` で呼びます。空の
  `[]` が addon sentinel への dispatch trigger です。
- **Pure-Taida facades** (ANSI 文字列、スタイリング、レンダラ mutation、
  行エディタ、ウィジェット) は **通常の関数呼び出し** `Name(...)` です。

完全なシンボル一覧と呼び出し方は [`docs/reference/`](../reference/) と
addon の README (`taida-lang/terminal` のリポジトリ側) を参照してください。

---

## 2. raw mode + alternate screen

ターミナル UI を構築する典型的なライフサイクル:

```taida
>>> taida-lang/terminal => @(RawModeEnter, RawModeLeave, AltScreenEnter, AltScreenLeave, Write, ClearScreen)

bytes1 <= Write[](AltScreenEnter())
RawModeEnter[]()

// ── ここで描画 / 入力ループ ──

RawModeLeave[]()
bytes2 <= Write[](AltScreenLeave())
```

- **`RawModeEnter[]()` / `RawModeLeave[]()`** — termios の cooked mode
  / raw mode を切り替えます。raw mode に入ると Enter / Backspace の
  自動処理が無効になり、`ReadEvent[]()` がキー / マウス / リサイズイベント
  を直接配信します。
- **`AltScreenEnter()` / `AltScreenLeave()`** — alternate screen buffer の
  enable / leave。スクロールバックを汚さずにフルスクリーン UI を出して
  終了時に元の端末状態へ戻せます。
- **`Write[](bytes)`** — `stdout` と違い末尾改行を付けず、書き込んだ
  バイト数を返します。ANSI control sequence を流し込む際は必ず `Write[]`
  を使ってください (改行が混じると framing が壊れます)。

`RawModeEnter` / `AltScreenEnter` を呼んだら **必ず対の Leave を呼ぶ**
責務はユーザ側にあります。例外パスでも端末状態が戻るよう、`|==` エラー
天井で leave を流すパターンを推奨します。

---

## 3. line editor — read line with editing

`LineEditor*` 系は **純粋な状態機械** として実装されています。1 イベント
受け取って次の状態を返す形なので、recursive helper で event loop を組み立てます。

```taida
>>> taida-lang/terminal => @(LineEditorNew, LineEditorRender, PromptOptions, PromptMode, Write)

opts <= PromptOptions(
  prompt <= "name> ",
  initial <= "Misato",
  placeholder <= "",
  mode <= PromptMode.Normal,
  history <= @[],
  completion <= @()
)

editor <= LineEditorNew(opts)
view <= LineEditorRender(editor)
bytes <= Write[](view.text)
```

| API | 用途 |
|-----|------|
| `LineEditorNew(opts)` | `PromptOptions` から初期状態を構築 |
| `LineEditorStep(state, event)` | 1 event を適用して次の `LineEditorState` を返す (純粋) |
| `LineEditorRender(state)` | 現在状態の ANSI 描画文字列を `@(text, ...)` で返す |
| `LineEditorAction` | enum pack — `Submitted` / `Cancelled` / `Editing` |
| `PromptMode` | enum pack — `Normal` / `Password` (入力 echo を伏せる) |

実用例は [`examples/terminal_line_editor.td`](../../examples/terminal_line_editor.td)
を参照。realistic な event loop は addon 側 README (`taida-lang/terminal`)
の line editor サンプルにフルバージョンがあります。

**recursion 注意**: Taida は同一スコープでの再代入を禁止しているため、
編集の各ステップごとに新しい名前 (`editor0`, `editor1`, …) を導入するか、
recursive 関数で state を threading してください。

---

## 4. spinner / progress bar / status line

CLI のフィードバック UI は 3 種類提供されています:

```taida
>>> taida-lang/terminal => @(SpinnerState, SpinnerNext, SpinnerRender, ProgressBar, ProgressOptions, StatusLine, Write, ClearLine)

// spinner
sp0 <= SpinnerState
sp1 <= SpinnerNext(sp0)
clear1 <= Write[](ClearLine())
out1 <= Write[](SpinnerRender(sp1))

// progress bar (50/100 = 50%)
bar <= ProgressBar(50, 100, ProgressOptions)
stdout(bar)

// status line (40 cells wide)
sl <= StatusLine("encoding", "12 MB / 30 MB", 40)
stdout(sl)
```

| API | 用途 |
|-----|------|
| `SpinnerState` / `SpinnerNext(s)` / `SpinnerRender(s)` | 1 frame ずつ進める純粋 state machine |
| `ProgressBar(current, total, opts)` | 進捗バー文字列を返す。`opts` (`ProgressOptions`) で幅 / フィル文字をカスタマイズ |
| `StatusLine(left, right, width)` | 左寄せ + 右寄せをパディングで揃えた幅固定行 |

完全例は [`examples/terminal_spinner.td`](../../examples/terminal_spinner.td)。

`Write[](ClearLine())` を毎フレーム前に流すことで「同じ行を上書き」表現が
得られます。`stdout` は末尾改行を付けてしまうので、フレーム重ね描きには
向きません — ここは必ず `Write[]` を使ってください。

---

## 5. ScreenBuffer + diff renderer

複雑な TUI (TUI editor / TUI dashboard) は仮想スクリーンバッファ + 差分
レンダラを使います。

```taida
>>> taida-lang/terminal => @(BufferNew, BufferWrite, RenderFrame, CellStyle, Write)

plain <= CellStyle(fg <= "", bg <= "", bold <= false, dim <= false, underline <= false, italic <= false)

prev <= BufferNew[](20, 5)
next <= BufferWrite(prev, 1, 1, "Hello, world", plain)

frame <= RenderFrame(prev, next)
out <= Write[](frame.text)
// 次フレームでは frame.next を新たな prev として使う
```

| API | 用途 |
|-----|------|
| `BufferNew[](cols, rows)` | 全 cell を空 (`" "`) で初期化したバッファを確保 (native) |
| `BufferResize[](buf, cols, rows[, fill])` | サイズ変更、cursor は新領域内に clamp (native) |
| `BufferPut(buf, col, row, cell)` | 1 セル書き換え (純粋、`Cell` を渡す) |
| `BufferWrite(buf, col, row, text, style)` | 文字列を `(col, row)` から右へ書き込む。右端で truncate |
| `BufferFillRect(buf, x, y, w, h, cell)` | 矩形領域を埋める |
| `BufferBlit(dst, src, dstX, dstY)` | 別バッファを貼り込む |
| `BufferClear(buf, fill?)` | 全 cell をリセット |
| `RenderFull(buf)` | バッファ全体を ANSI 文字列で 1 度に描画 |
| `BufferDiff(prev, next)` | 2 バッファの差分 op リスト (`@[DiffOp]`) を返す |
| `RenderOps(ops)` | diff op リストを ANSI 文字列に展開 |
| `RenderFrame(prev, next)` | `BufferDiff` + `RenderOps` を 1 関数にまとめ、`@(text, next)` を返す。サイズ変化時は自動で `RenderFull` にフォールバック |

**diff renderer の効用**: 30Hz UI でも書き込みは「差分セルの ANSI 制御
シーケンス」だけ。フルフレーム文字列を毎回送るより 1〜2 桁少ないバイト数
で済みます。

`Cell` / `CellStyle` の 6 フィールド (`fg` / `bg` / `bold` / `dim` /
`underline` / `italic`) は **全部書く必要があります** (省略不可)。未指定の
色は `""`、未指定の属性は `false` を渡してください。

---

## 6. Unicode display width

ターミナル UI でテキストを揃えるには **表示セル幅** が必要です。Taida の
`Str.length()` は **コードポイント数** (UTF-8 を decode した結果の char
count) を返すだけで、CJK / 絵文字の **2 セル幅** や Combining Mark の
**0 セル幅** を考慮しません。

| API | 用途 |
|-----|------|
| `DisplayWidth(text)` | 文字列全体の表示セル幅。ASCII = 1 / CJK = 2 / Combining Mark = 0 |
| `MeasureGrapheme(text)` | 単一 grapheme cluster の幅と width mode (`Narrow` / `Wide` / `Zero` / `Ambiguous`) |
| `NormalizeCellText(text)` | 空 → `" "`, 制御文字を除去, TAB → 4 spaces。cell に入る前の正規化 |
| `TruncateWidth(text, width)` | 右端を表示幅で切り詰める (CJK 境界を尊重) |
| `PadWidth(text, width)` | 表示幅に合わせて右側を空白パディング |
| `WidthMode` | enum pack — `Narrow=0` / `Wide=1` / `Zero=2` / `Ambiguous=3` |

```taida
>>> taida-lang/terminal => @(DisplayWidth, PadWidth, TruncateWidth)

stdout(DisplayWidth("hello").toString())   // 5
stdout(DisplayWidth("漢字").toString())     // 4
stdout(PadWidth("hi", 5))                   // "hi   "
stdout(TruncateWidth("abcdef", 3))          // "abc"
```

**East Asian Ambiguous の扱い**: `MeasureGrapheme` は `WidthMode.Ambiguous`
を返します。Ambiguous 文字 (`α`, `±`, ギリシャ・キリル etc.) は端末の
font / locale 設定で 1 / 2 セルどちらにもなり得るため、`DisplayWidth` は
**1 セル扱い** で固定しています (大部分の terminal で安全な側)。CJK
Locale で 2 セル扱いが必要な場合は addon 側の WidthMode を見て分岐して
ください。

**制御文字 / NUL**: `NormalizeCellText` がそれらを除去 / 置換するため、
ScreenBuffer に格納する前に通すのが安全です。

---

## 7. mouse / key event handling

raw mode + `MouseTrackingEnter()` を有効にすると、`ReadEvent[]()` から
key / mouse / resize event が統一形式で取れます。

```taida
>>> taida-lang/terminal => @(RawModeEnter, RawModeLeave, ReadEvent, EventKind, MouseKind, MouseTrackingEnter, MouseTrackingLeave, Write)

bytes1 <= Write[](MouseTrackingEnter())
RawModeEnter[]()

event <= ReadEvent[]()
report <= (
  | event.kind == EventKind.Key    |> "key " + event.key.text
  | event.kind == EventKind.Mouse  |> "mouse " + event.mouse.kind.toString() + " @ (" + event.mouse.col.toString() + "," + event.mouse.row.toString() + ")"
  | event.kind == EventKind.Resize |> "resize " + event.resize.cols.toString() + "x" + event.resize.rows.toString()
  | _ |> "unknown"
)
stdout(report)

RawModeLeave[]()
bytes2 <= Write[](MouseTrackingLeave())
```

**`EventKind` enum**:

| variant | `event.<...>` |
|---------|---------------|
| `EventKind.Key` | `event.key` (`@(kind, text, ctrl, alt, shift)`) |
| `EventKind.Mouse` | `event.mouse` (`@(kind, col, row, ctrl, alt, shift)`) |
| `EventKind.Resize` | `event.resize` (`@(cols, rows)`) |

**`MouseKind` enum** (代表的な variant):

| variant | 意味 |
|---------|------|
| `MouseKind.LeftDown` / `MouseKind.LeftUp` | 左クリック down / up |
| `MouseKind.MiddleDown` / `MouseKind.MiddleUp` | 中クリック down / up |
| `MouseKind.RightDown` / `MouseKind.RightUp` | 右クリック down / up |
| `MouseKind.Move` | カーソル移動 (button 押下無し) |
| `MouseKind.Drag` | ドラッグ (button 押下中の移動) |
| `MouseKind.ScrollUp` / `MouseKind.ScrollDown` | ホイール up / down |

**`KeyKind` enum** は v1 ABI で 28 variant が pin されています
(`Char` / `Enter` / `Escape` / `Tab` / `Backspace` / `Delete` /
`ArrowUp..ArrowRight` / `Home` / `End` / `PageUp` / `PageDown` / `Insert` /
`F1..F12` / `Unknown`)。タグ値の追加・並び替えは ABI bump 必須です。

実用的な mouse demo は [`examples/terminal_mouse.td`](../../examples/terminal_mouse.td)
を参照。SGR 1006 (`\x1b[<...M`) extended mouse encoding を使うため、
ほぼ全モダン端末 (xterm / kitty / iTerm2 / WezTerm / Alacritty / Windows
Terminal 等) で動作します。

---

## 8. backend サポート

| Backend | サポート | 備考 |
|---------|---------|------|
| Interpreter (with `feature = "native"`) | ○ | Default build。addon ABI 経由で cdylib に dispatch |
| Native (AOT) | ○ | `src/addon/facade.rs` で facade を静的解析、IR に lower |
| JS | × | addon dispatcher が無いため `>>> taida-lang/terminal` 自体が deterministic error。stdout 系の代替は `taida-lang/os` |
| WASM (`wasm-min` / `wasm-wasi` / `wasm-edge` / `wasm-full`) | × | addon dispatcher 未提供。`docs/STABILITY.md` §1.2 参照 |

addon の cdylib build と publish 手順は [`docs/guide/13_creating_addons.md`](13_creating_addons.md)
を参照してください。`taida-lang/terminal` 自身もこの publish パイプライン
で release されています。

詳細な API シグネチャは addon 側の README + `taida doc generate` 出力
(`docs/api.md` 相当) を参照してください — `docs/reference/` 配下に terminal
個別の API ファイルは置きません (bundled package docs governance、
addon 側で自己完結する原則)。
