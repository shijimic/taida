/*
 * native_runtime/runtime.h — C13-4 public runtime declaration index
 *
 * C13-4 (C13B-004) では `native_runtime.c` 分割を C12B-026 で導入された 7
 * フラグメント連結から、設計書 `.dev/C13_DESIGN.md §C13-4` が指定する 5
 * 責務ファイル + 共有ヘッダに再配置した。
 *
 * 本ヘッダは **C translation unit には include しない** — clang に渡される
 * C ソースは `NATIVE_RUNTIME_C`（`mod.rs`）が 5 ファイルを連結して生成する
 * 単一ストリームのままであり、各 `.c` ファイルは自己完結した forward
 * declaration を先頭で保持している。
 *
 * このヘッダは以下の 2 つの役割だけを持つ:
 *
 *   1. **責務境界の宣言的 index**: どの `.c` ファイルがどの公開 runtime 関数
 *      群を提供するかを中央一箇所で明記し、C13-5 以降の保守 / レビューから
 *      責務を俯瞰できるようにする。
 *   2. **connector の ABI 契約を自己ドキュメント化**: `src/codegen/emit.rs`
 *      が emit する `extern` 宣言と 1 対 1 で対応する名前群を列挙する。
 *      C13-4 時点ではこのヘッダから ABI を移動していないため、宣言の更新
 *      は従来通り emit.rs 側で行う。
 *
 * C13 非交渉条件 §5 に従い、このヘッダの導入は機械的な境界の明示のみであり、
 * runtime コードの振る舞いを変更しない。連結総バイト長は C12B-026 と同じ
 * 886,457 bytes を保つ (`mod.rs` の `EXPECTED_TOTAL_LEN` 不変条件)。
 *
 * ─────────────────────────────────────────────────────────────────────────
 * 責務境界表 (C13-4a)
 * ─────────────────────────────────────────────────────────────────────────
 *
 * core.c           — C runtime 基盤
 *   - libc stubs / safe-malloc / allocator / type conversion molds
 *   - ref-counting / heap strings / BuchiPack / globals
 *   - Closure / List / Bytes / String / Regex / polymorphic dispatchers
 *   - template strings / Int/Float/Bool/Num methods
 *   - HashMap / Set / polymorphic length / collection methods
 *   - Error ceiling (setjmp/longjmp) / Result / Lax methods
 *   - polymorphic monadic dispatch / Async pthread support
 *   - Debug for list / JSON Molten Iron / stdlib math
 *   - Field registry / jsonEncode/jsonPretty / stdlib I/O / SHA-256
 *   (旧 01_core.inc.c + 02_error_json.inc.c、C12B-026 における fragment 1+2)
 *
 * os.c             — taida-lang/os package
 *   - Read / readBytes / ListDir / Stat / Exists / EnvVar
 *   - writeFile / writeBytes / appendFile / remove / createDir / rename
 *   - run / execShell / allEnv / ReadAsync
 *   (旧 03_os.inc.c、C12B-026 における fragment 3)
 *
 * tls.c            — NET5-4a OpenSSL dlopen + TLS / TCP + pool
 *   - OpenSSL libssl/libcrypto dlopen シンボルテーブル
 *   - TLS-aware I/O wrappers (read/write)
 *   - HTTP/1.1 over raw TCP （TLS 無しの素の socket 経路）
 *   - TCP socket APIs / pool package runtime
 *   (旧 04_tls_tcp.inc.c、C12B-026 における fragment 4)
 *
 * net_h1_h2.c      — HTTP/1 + WebSocket + HTTP/2 server
 *   - taida-lang/net HTTP v1 runtime
 *     (httpParseRequestHead / httpEncodeResponse / readBody /
 *      keep-alive / chunked compaction / httpServe helpers)
 *   - v3 streaming writer / v4 body streaming / WebSocket
 *     (wsUpgrade / wsSend / wsReceive / wsClose) / thread pool / worker thread
 *   - Native HTTP/2 server (NET6-3a h2 parity)
 *     (HPACK static & dynamic tables / HPACK Huffman /
 *      HPACK int/string coding / H2 stream state / H2 frame I/O /
 *      H2 response send / H2 frame processing /
 *      H2 request & response extraction / serve one connection /
 *      taida_net_h2_serve)
 *   (旧 05_net_v1.inc.c + 06_net_h2.inc.c、C12B-026 における fragment 5+6)
 *
 * net_h3_quic.c    — HTTP/3 + QPACK + QUIC + httpServe entry + main
 *   - H3/QPACK constants / QPACK static & dynamic tables
 *   - QPACK int/string/header coding / H3 stream state
 *   - H3 varint / H3 frame I/O / SETTINGS / GOAWAY
 *   - H3 request/response path
 *   - NET7-8a libquiche dlopen FFI
 *   - QPACK encoder/decoder instruction streams / H3 self-tests
 *   - NET7-8b QUIC connection pool / serve_h3_loop / taida_net_h3_serve
 *   - httpServe entry / RC2.5 addon dispatch / main()
 *   (旧 07_net_h3_main.inc.c、C12B-026 における fragment 7)
 *
 * ─────────────────────────────────────────────────────────────────────────
 * 連結順序 (NATIVE_RUNTIME_C invariant)
 * ─────────────────────────────────────────────────────────────────────────
 *
 *   core.c  ->  os.c  ->  tls.c  ->  net_h1_h2.c  ->  net_h3_quic.c
 *
 * この順序は旧 fragment 1..7 の連結順と完全一致するため、DCE / static
 * helper cross-reference / forward declaration の可視範囲は
 * C12B-026 以前とバイト単位で同一である。
 */
