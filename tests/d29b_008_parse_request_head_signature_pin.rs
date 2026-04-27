//! D29B-008 (Lock-E E1) — `parse_request_head` の関数 signature を string match
//! で pin する CI test。
//!
//! このテストは `src/interpreter/net_eval/helpers.rs` を読み込んで、
//! `parse_request_head` が **`pub(crate) fn parse_request_head(bytes: &[u8]) -> Value`**
//! という正確な signature を保っていることを assert する。
//!
//! 何故 pin が必要か:
//! - `build_parse_result` は httparse が返す `&str` / `&[u8]` の subslice から
//!   span を再構築するために `bytes_subslice_offset(bytes, sub)` を使う
//! - この helper は `bytes` と subslice が同じ allocation を共有することを
//!   pointer range check で前提とする
//! - もし将来 `parse_request_head` の signature が `Arc<Vec<u8>>` を取るように
//!   変更され、内部で clone されると、httparse に渡る bytes と
//!   `bytes_subslice_offset` に渡す bytes が **異なる allocation** になり、
//!   range check が常に None を返して span が **silent に 0/0** になる
//! - これは silent bug で test runner からは PASS に見えるが NET routing が
//!   全壊する致命的 regression
//!
//! 本 test は signature の任意の変更を CI で red にし、変更時に
//! `bytes_subslice_offset` の不変性を再評価することを強制する。

use std::fs;
use std::path::PathBuf;

const HELPERS_RELATIVE: &str = "src/interpreter/net_eval/helpers.rs";

fn read_helpers() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(HELPERS_RELATIVE);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("D29B-008: failed to read {}: {}", path.display(), e))
}

#[test]
fn d29b_008_parse_request_head_signature_is_pinned() {
    let src = read_helpers();
    let needle = "pub(crate) fn parse_request_head(bytes: &[u8]) -> Value {";
    assert!(
        src.contains(needle),
        "D29B-008 (Lock-E E1): parse_request_head signature must be exactly:\n  \
         {}\n\n\
         Detected change in {}. Changing this signature breaks the safe-subslice \
         invariant in build_parse_result (bytes_subslice_offset relies on the \
         input bytes and httparse's subslices sharing the same allocation). If \
         you intentionally changed the signature, you MUST re-audit \
         bytes_subslice_offset and update this pin together.",
        needle,
        HELPERS_RELATIVE
    );
}

#[test]
fn d29b_008_parse_request_head_signature_pin_doc_marker_present() {
    let src = read_helpers();
    assert!(
        src.contains("SIGNATURE PIN (D29B-008)"),
        "D29B-008 (Lock-E E1): parse_request_head must carry the \
         `SIGNATURE PIN (D29B-008)` doc-comment marker explaining why the \
         signature is locked. The marker was removed from {}.",
        HELPERS_RELATIVE
    );
}

#[test]
fn d29b_008_bytes_subslice_offset_helper_present() {
    let src = read_helpers();
    let needle = "fn bytes_subslice_offset(haystack: &[u8], needle: &[u8]) -> Option<usize>";
    assert!(
        src.contains(needle),
        "D29B-008 (Lock-E E2): the safe subslice helper:\n  {}\n\
         must be defined in {}. This helper replaces the raw \
         `as_ptr() as usize - base` pointer arithmetic that was vulnerable \
         to silent bugs under future Arc-based refactors.",
        needle,
        HELPERS_RELATIVE
    );
}

#[test]
fn d29b_008_no_legacy_pointer_arithmetic_in_build_parse_result() {
    let src = read_helpers();
    // The legacy pattern was: `let base = bytes.as_ptr() as usize;` immediately
    // before the body of build_parse_result. We assert the literal anti-pattern
    // is gone; if a legitimate `bytes.as_ptr()` appears elsewhere it is fine
    // (bytes_subslice_offset itself uses pointer comparison, but in a checked
    // form, not as a raw `as usize - base` subtraction).
    let antipattern = "let base = bytes.as_ptr() as usize;";
    assert!(
        !src.contains(antipattern),
        "D29B-008 (Lock-E E2): the legacy pointer-arithmetic anti-pattern:\n  {}\n\
         was reintroduced into {}. Use `bytes_subslice_offset` instead.",
        antipattern,
        HELPERS_RELATIVE
    );
}
