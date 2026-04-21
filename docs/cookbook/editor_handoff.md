# エディタ連携パターン

外部エディタ（nvim / vim / micro 等）を Taida プログラムから起動し、ユーザーの編集結果を受け取るための典型パターンをまとめます。

前提 API: [`runInteractive`](../reference/os_api.md#22-interactive-版c19-以降)（C19 以降）

---

## 1. 最小パターン: 一時ファイル経由のやりとり

プログラムが生成した初期内容をエディタで編集させて、保存結果を読み直すパターン。CLI ツールで `$EDITOR` を起動する最も基本的な形です。

```taida
>>> taida-lang/os => @(writeFile, Read, runInteractive, EnvVar, remove)

// 1) 初期内容を一時ファイルに書き出す
path    <= "/tmp/taida_edit_draft.md"
initial <= "# Draft\n\nWrite here...\n"
writeFile(path, initial)

// 2) エディタを起動（$EDITOR 優先、無ければ nvim）
editor  <= EnvVar["EDITOR"]().getOrDefault("nvim")
r       <= runInteractive(editor, @[path])

// 3) exit code を検査（失敗は早期終了、成功は通過）
| !r.hasValue |>
  bytes <= stderr("editor failed: " + r.__error.message)
  ><
| r.__value.code != 0 |>
  bytes <= stderr("editor exited non-zero")
  ><

// 4) 編集後の内容を読む
edited <= Read[path]()
| edited.hasValue |> stdout(edited.__value)
| _               |> stderr("read back failed")

// 5) 一時ファイルを片付ける
remove(path)
```

---

## 2. `$EDITOR` 解決の優先順位

慣習として `$VISUAL > $EDITOR > fallback` の順で解決します。CLI ツールはこの順序を守ると他 UNIX ツールと挙動が揃います。

```taida
>>> taida-lang/os => @(EnvVar)

pickEditor =
  visual <= EnvVar["VISUAL"]()
  editor <= EnvVar["EDITOR"]()
  // C20 / E0303: 複数行の rhs 多アーム条件は丸括弧で包む。
  picked <= (
    | visual.hasValue |> visual.__value
    | editor.hasValue |> editor.__value
    | _                 |> "nvim"
  )
  picked
=> :Str
```

---

## 3. Raw mode を使う TUI からエディタを呼ぶ

自前の TUI が `taida-lang/terminal` アドオンで raw mode に入っている場合、エディタに端末を渡す前に **必ず raw mode を抜ける** こと。抜けずに `runInteractive` すると、エディタが echo off / canonical off の状態で動き、キー入力が期待通りに届きません。

```taida
>>> taida-lang/os       => @(runInteractive)
>>> taida-lang/terminal => @(RawModeEnter, RawModeLeave, AltScreenEnter, AltScreenLeave)

// ... TUI ループ ...
// e キーが押された時:

stdout(AltScreenLeave())    // alternate screen を抜ける (ANSI 文字列を書き出す)
RawModeLeave[]()            // cooked mode に戻す（必須）

r <= runInteractive(editor, @[path])

RawModeEnter[]()            // TUI モードに復帰
stdout(AltScreenEnter())

| r.hasValue && r.__value.code == 0 |> reloadBuffer(path)
```

`AltScreenEnter` / `AltScreenLeave` は ANSI エスケープ文字列を返す関数で、`stdout(...)` に渡して端末へ書き出します。`RawModeEnter` / `RawModeLeave` はモールドとして呼び出し、実際のモード切替を副作用で行います。

**落とし穴**:
- `RawModeLeave` / `AltScreenLeave` は `runInteractive` の前で呼ぶ（後では遅い）
- エディタが panic して raw mode のまま戻ると、端末が壊れた状態になる → `runInteractive` 戻り直後に無条件で `RawModeEnter[]()` を再実行して整合を取る

---

## 4. エディタ exit code のハンドリング

エディタの exit code は「編集結果を採用するか」の 1 次判定に使えます。ただし規約はエディタによって異なるので、`$EDITOR` に任せる実装では保守的に扱います。

| exit code | 典型的な意味 |
|-----------|--------------|
| 0 | 正常終了（保存して終了）|
| 非 0 | 中断・エラー（vim `:cq`、nvim panic 等） |

```taida
| !r.hasValue |>
  bytes <= stderr("editor failed: " + r.__error.message)
  ><

code <= r.__value.code
| code == 0 |> stdout("accepted: " + path)
| _ |>
  bytes <= stderr("edit cancelled")
  stdout("kept original")
```

**ポイント**: 失敗ガードと結果ガードを **別々のチェーン** に分けます。間に `<=` 束縛を 1 つ挟むと、2 つの `|` が別のチェーンとして処理されるので、戻り値型の不整合（失敗の `><` と成功分岐の `Str`/`Int`）を避けられます。

単純な 0/非 0 判定を超える（例: nvim の `:cq 1` でカスタム intent を渡す）運用は脆いので推奨しません。代わりに「**保存ファイルの内容を diff で比較する**」のが堅実です:

```taida
>>> taida-lang/os => @(Read, runInteractive)

before <= Read[path]().getOrDefault("")
r      <= runInteractive(editor, @[path])
after  <= Read[path]().getOrDefault("")

| before == after |> stdout("no changes")
| _                |> applyChanges(after)
```

---

## 5. エディタ起動前の対話プロンプト（C20-2 以降）

`runInteractive` / `execShellInteractive` に渡す前の「どのファイルを編集しますか？」「コミットメッセージのドラフトを書きますか？」のようなプロンプトは、`stdinLine` を使って UTF-8-aware に受け取ると日本語 / 中国語 / 絵文字入力が編集中に壊れません。従来の `stdin` は kernel cooked mode のため Backspace が 1 バイト単位で働き、multibyte 文字が途中まで残る不具合 (ROOT-7) がありました。

```taida
>>> taida-lang/os => @(runInteractive, writeFile, Read, EnvVar)

// ── プロンプト部は stdinLine（UTF-8 対応）──
stdinLine("タイトル: ") ]=> titleLax
title <= titleLax.getOrDefault("（無題）")

path <= "/tmp/taida_editor_draft_" + title + ".md"
writeFile(path, "# " + title + "\n\n")

// ── エディタ起動は runInteractive（TTY passthrough）──
editor <= EnvVar["EDITOR"]().getOrDefault("nvim")
r      <= runInteractive(editor, @[path])

| !r.hasValue |> stderr("editor failed")
| _           |> stdout(Read[path]().getOrDefault(""))
```

**注意**:

- `stdinLine` は raw mode を内部で一時的に有効化してから戻します。呼び出し直後に `RawModeEnter[]()` を再実行したい TUI は、`stdinLine ]=> line` の直後に入れてください（`runInteractive` との順序は 4 節と同じ）。
- `stdinLine` の戻り型は `Async[Lax[Str]]` なので `]=>` で unmold し、**その中の Lax** を `.getOrDefault("")` で取り出します。`<= stdinLine(...)` と書くと Async がそのまま変数に入るだけで line は取れません。

---

## 5. アンチパターン

### 5.1 captured `run` でエディタを起動する

```taida
// NG: nvim が起動しても画面に何も出ない
r <= run("nvim", @["/tmp/edit.md"])
```

`run` は stdout / stderr を pipe で奪い、stdin も pipe にリダイレクトします。TTY を要求するエディタは機能不全に陥ります。**TUI を起動するときは必ず `runInteractive`** を使うこと。

### 5.2 Raw mode を抜けずに handoff

TUI の表示が崩れるだけでなく、エディタ側のキーバインド（特に Ctrl-C / Ctrl-Z / Backspace）が壊れます。`runInteractive` の前に `RawModeLeave[]()` を呼ぶのを忘れない。

### 5.3 `execShellInteractive` を常用する

```taida
// 避けたい: shell 展開が不要なのに sh -c を挟む
execShellInteractive("nvim " + path)   // shell injection リスクあり
```

`path` にスペースや特殊文字が入ると簡単に壊れます。shell 展開が本当に必要でない限り `runInteractive(prog, args)` を使い、引数を argv として個別に渡してください。

---

## 6. 関連リンク

- API リファレンス: [`docs/reference/os_api.md`](../reference/os_api.md)
- ガイド: [`docs/guide/14_os_package.md`](../guide/14_os_package.md)
- Taida terminal アドオン: `taida-lang/terminal`（raw mode / alternate screen など）
