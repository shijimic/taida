# API リファレンス

`docs/api/` は Taida のパッケージ API リファレンスを集めた場所です。
コア同梱パッケージのインポート方法、公開シンボル、シグネチャ、戻り値、
失敗条件、バックエンド対応範囲などを扱います。

言語仕様そのもの (演算子、型システム、診断コードなど) は
[`docs/reference/`](../reference/) を、ナラティブな学習用ドキュメントは
[`docs/guide/`](../guide/) を参照してください。

---

## ファイル一覧

| ファイル | 役割 |
|----------|------|
| [`prelude.md`](prelude.md) | プレリュード関数 (`stdout` / `stdin` / `nowMs` / `sleep` / `jsonEncode` / `debug` / `range` / `exit` 等) + インポート不要モールド一覧 |
| [`os.md`](os.md) | `taida-lang/os` の API 仕様 (ファイル I/O、プロセス、環境、ソケット、DNS) |
| [`net.md`](net.md) | `taida-lang/net` の API 仕様 (HTTP/1.1、H2、H3、WebSocket、SSE) |
| [`crypto.md`](crypto.md) | `taida-lang/crypto` の API 仕様 (`sha256`) |
| [`js.md`](js.md) | `taida-lang/js` の API 仕様 (JS 相互運用の実行記述群) |
| [`pool.md`](pool.md) | `taida-lang/pool` の API 仕様 (リソースプーリング) |
| [`build_descriptors.md`](build_descriptors.md) | `taida-lang/build` の API 仕様 (`BuildUnit` / `BuildPlan` / `AssetBundle` / `RouteAsset` / `BuildHook`) |

---

## パッケージ層の構造

Taida のパッケージは大きく次の 3 層に分かれます。

1. **プレリュード** — インポート不要で常に利用可能な関数・型コンストラクタ。
   詳細は [`prelude.md`](prelude.md) を参照してください。
2. **コア同梱パッケージ** — `taida-lang/os` / `taida-lang/net` /
   `taida-lang/crypto` / `taida-lang/js` / `taida-lang/pool` /
   `taida-lang/build`。Taida バイナリに同梱されており、`taida ingot install`
   などのインストールは不要です。`>>> taida-lang/<pkg> => @(...)` で
   明示インポートします (`taida-lang/build` のディスクリプタは
   ランタイム値ではなくビルドドライバ専用値として扱われます)。本ディレクトリは
   主にこの層を扱います。
3. **公式アドオン** — `taida-lang/terminal` など。ネイティブ cdylib を
   `taida ingot install` で取得するインゴットとして配布されます。
   詳細は [アドオン作成ガイド](../guide/13_creating_addons.md) と
   [アドオンマニフェスト](../reference/addon_manifest.md) を参照
   してください。

個別パッケージの API 仕様 (`os.md` / `net.md` / `crypto.md` /
`js.md` / `pool.md` / `build_descriptors.md`) を直接参照してください。
