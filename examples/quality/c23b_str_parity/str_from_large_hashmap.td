// C23B-008 proactive audit: a HashMap that crosses the load-factor
// resize boundary (>= 0.7 for wasm, >= 0.75 for native) must still
// preserve insertion order in its rebuilt layout. We insert 16 keys
// ("k0".."k15") in order — wasm's default cap is 16 so this triggers
// exactly one resize at k11 (16 * 0.7 = 11.2 → resize when len*10 >=
// cap*7). The `taida_hashmap_resize` / `_wasm_hashmap_new_with_cap`
// path now walks the OLD order array and re-inserts in the same
// order, stamping the NEW order array as it goes.
m <= hashMap().set("k0", 0).set("k1", 1).set("k2", 2).set("k3", 3).set("k4", 4).set("k5", 5).set("k6", 6).set("k7", 7).set("k8", 8).set("k9", 9).set("k10", 10).set("k11", 11).set("k12", 12).set("k13", 13).set("k14", 14).set("k15", 15)
stdout(Str[m]())
