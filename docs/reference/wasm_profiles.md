# WASM プロファイルリファレンス

このリファレンスは、Taida が受け付ける WebAssembly ターゲット名と、
各プロファイルで利用できる機能の境界を定義します。ここに掲載する
プロファイル名は `taida build` が受け付ける公開仕様の一部であり、
新規プロファイルの追加・既存プロファイルの意味変更は世代をまたぐ
変更として扱います。

コマンド構文は `docs/reference/cli.md` を、複数ターゲットを束ねる
ビルド構成の互換性は `docs/reference/build_descriptors.md` を参照
してください。

---

## プロファイル名

| プロファイル | 目的 | 実行環境の前提 |
|--------------|------|----------------|
| `wasm-min` | 最小構成の移植可能な WebAssembly 出力 | WASI ホスト関数およびアドオンディスパッチャを利用しません |
| `wasm-wasi` | WASI 上で動作するコマンド／ランタイム向け出力 | 明示的に対応した範囲で WASI preview1 のホスト関数を利用します |
| `wasm-edge` | エッジランタイム向けの WebAssembly 出力 | 明示的に対応したエッジホスト機能のみを利用します |
| `wasm-full` | Taida の完全機能版 WASM プロファイル | WASI ランタイムに加え、ホスト経由のアドオン呼び出しを利用できます |

これらのプロファイル名は別名ではありません。あるパッケージや API が
`wasm-full` で動作するからといって、`wasm-min` / `wasm-wasi` /
`wasm-edge` でも自動的に動作するわけではありません。

---

## アドオン呼び出し

アドオンに裏打ちされたインポートは `wasm-full` でのみ利用できます。
`wasm-min` / `wasm-wasi` / `wasm-edge` はアドオンディスパッチャを
持たないため、該当インポートは決定的な診断とともにコンパイル時に
拒否されます。

WASM からアドオンを呼び出すマニフェストでは、`native/addon.toml` の
`targets` に `"wasm-full"` を明示してください。マニフェスト側の
許可リストと、非対応バックエンドに対する診断文の仕様は
`docs/reference/addon_manifest.md` を参照してください。

---

## コアパッケージの互換性

taida 本体にバンドルされるコア同梱パッケージの互換性はプロファイルごとに次のとおりです。

| パッケージ領域 | `wasm-min` | `wasm-wasi` | `wasm-edge` | `wasm-full` |
|----------------|------------|-------------|-------------|-------------|
| `taida-lang/os` | 利用不可 | 文書化済みの WASI 向け部分集合 | 文書化済みのエッジ向け部分集合 | `wasm-wasi` と同一の OS 部分集合 |
| `taida-lang/net` | 利用不可 | 文書化済みの WASI 向け部分集合 (plaintext HTTP/1.1 `httpServe`) | 利用不可 | `wasm-wasi` と同一の net 部分集合 |
| `taida-lang/crypto` | 利用不可 | 利用不可 | 利用不可 | 対応 |
| `taida-lang/js` | 利用不可 | 利用不可 | 利用不可 | 利用不可 |
| `taida-lang/pool` | 利用不可 | 利用不可 | 利用不可 | 全 API |
| `taida-lang/build` (ディスクリプタ) | reject | `EnvVar` / `allEnv` / `Read` / `Exists` / `writeFile` / `readBytesAt` のみ受理 | `EnvVar` / `allEnv` のみ受理 | `wasm-wasi` と同一の OS subset |
| アドオンに裏打ちされたパッケージ | 利用不可 | 利用不可 | 利用不可 | マニフェストが明示的に対応宣言した場合のみ利用可 |

各パッケージ内の API 単位の対応範囲は、対応する `docs/api/<package>.md`
を参照してください。`taida-lang/build` のディスクリプタが各ターゲットで
受理する OS API のサブセットは、`docs/api/build_descriptors.md` の
ターゲット別コア API 互換性表に網羅されています。

公式アドオン (例: `taida-lang/terminal`) は taida 本体にバンドルされて
おらず、各アドオンの WASM 対応はそれぞれのマニフェストに従います。
対応宣言の書式は `docs/reference/addon_manifest.md` を参照してください。

`wasm-wasi` / `wasm-full` の net 部分集合は、WASI preview1 の継承 fd
を使う plaintext HTTP/1.1 `httpServe` です。host が
`wasi_snapshot_preview1.sock_accept` を実装している場合は fd 3 の inherited
listener を使います。Wasmtime の legacy preview1 実行環境など
`sock_accept` を提供しない host では、accept 済み TCP 接続を fd 0/1
に接続する socket-activation 形式で 1 request を処理します。
この部分集合では guest が bind/listen を行わないため `httpServe` の
`port` は host 側 listener の選択には使われません。`timeoutMs` と
`maxConnections` も host の WASI socket 実装に委譲され、Taida runtime
側では追加の scheduling policy を持ちません。TLS 設定は非空 pack
なら compile-time reject されます。

---

## バックエンド間の挙動

言語意味論の基準実装はインタプリタです。WASM プロファイルは
インタプリタと同一の挙動を返すか、ターゲットに必要な機能が存在
しない場合に決定的な診断とともにコンパイル時に拒否します。

あるプロファイルから別のプロファイルへ実行時に暗黙にフォールバック
することはありません。
