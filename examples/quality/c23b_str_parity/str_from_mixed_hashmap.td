// C23B-007 repro (HashMap variant): mixed-value HashMap `.set("a", 1)
// .set("b", "x").set("c", 2)` must keep the middle entry's string
// value quoted. The WASM runtime previously downgraded hm[2] to UNKNOWN
// after the second .set() but then re-promoted back to INT on the
// third .set(), causing the renderer to emit the pointer of "x" as an
// integer (`1058`) under the Int fast path.
// Fix: `taida_hashmap_set_value_tag` latches on HETEROGENEOUS(-2).
m <= hashMap().set("a", 1).set("b", "x").set("c", 2)
stdout(Str[m]())
