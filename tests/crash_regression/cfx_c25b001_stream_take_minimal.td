// C25B-012 / C25B-001: minimal Stream-lowering regression guard.
// After commit 4e17e89 ("minimal Stream lowering for native/wasm →
// 4-backend parity") all three backends agree on the `Str[Stream]()`
// rendering shape. Before that fix the `STREAM_ONLY_FIXTURES` skip
// list in tests/c23_str_parity.rs hid this case; it is now empty
// and the minimal reproduction below pins the new 3-backend parity
// at the crash-regression layer so a regression surfaces even if
// c23_str_parity's Stream case is renamed or moved later.
s <= Stream[@[1, 2, 3]]()
stdout(Str[s]())
