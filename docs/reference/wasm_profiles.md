# WASM プロファイルリファレンス

このリファレンスは、Taida が受け付ける WebAssembly ターゲット名と、
各ターゲットで利用できる機能の境界を定義します。ここに載る
プロファイル名は、`taida build` が受け付ける公開仕様の一部です。

コマンド構文は `docs/reference/cli.md` を参照してください。複数の
ターゲットを組み合わせるビルドディスクリプタの互換性は
`docs/reference/build_descriptors.md` にあります。

---

## プロファイル名

| プロファイル | 目的 | 実行環境の前提 |
|--------------|------|----------------|
| `wasm-min` | 最小構成の移植可能な WebAssembly 出力 | WASI の host API とアドオン dispatcher を使わない |
| `wasm-wasi` | WASI 向けのコマンド / ランタイム出力 | 明示的に対応済みの範囲で WASI preview1 の host API を使う |
| `wasm-edge` | edge runtime 向けの WebAssembly 出力 | 明示的に対応済みの edge host 機能だけを使う |
| `wasm-full` | Taida の full WASM profile | WASI 向け runtime に加え、対応済みの host 経由アドオン呼び出しを使える |

これらのプロファイル名は別名ではありません。あるパッケージや API が
`wasm-full` に対応していても、`wasm-min`、`wasm-wasi`、`wasm-edge` に
自動的に対応するわけではありません。

---

## アドオン呼び出し

アドオンに裏打ちされた import は、`wasm-full` でのみ利用できます。
`wasm-min`、`wasm-wasi`、`wasm-edge` はアドオン dispatcher を提供しないため、
そのような import をコンパイル時に決定的な診断で拒否します。

WASM からアドオンを呼び出す manifest では、`native/addon.toml` の
`targets` に `"wasm-full"` を明示します。manifest 側の許可リストと、
非対応 backend に対する診断文は `docs/reference/addon_manifest.md` を
参照してください。

---

## core package 互換性

core package の互換性はターゲットごとに異なります。

| package area | `wasm-min` | `wasm-wasi` | `wasm-edge` | `wasm-full` |
|--------------|------------|-------------|-------------|-------------|
| `taida-lang/os` | 拒否 | 文書化済みの WASI 向け subset | 文書化済みの edge 向け subset | `wasm-wasi` と同じ OS subset |
| `taida-lang/net` | 拒否 | 拒否 | 拒否 | 拒否 |
| `taida-lang/terminal` | 拒否 | 拒否 | 拒否 | 拒否 |
| アドオンに裏打ちされた package | 拒否 | 拒否 | 拒否 | manifest が opt in した場合のみ対応 |

OS API の symbol 単位の subset は `docs/reference/build_descriptors.md` と
`docs/reference/os_api.md` にあります。NET API の拒否方針は
`docs/reference/net_api.md` にあります。

---

## backend 間の挙動

言語意味論の基準実装は interpreter です。WASM profile は interpreter と
同じ挙動を返すか、ターゲットに必要な機能がない場合にコンパイル時の
決定的な診断で拒否します。

ある WASM profile から別の WASM profile へ黙って切り替えることはありません。
