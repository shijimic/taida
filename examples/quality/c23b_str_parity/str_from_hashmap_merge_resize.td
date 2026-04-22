// C23B-008 reopen: merge that crosses the 0.75 load-factor resize
// boundary (native/wasm initial cap = 16; at 12 entries the table
// doubles to 32). `a.merge(b)` here produces 16 entries with two
// overlap keys (`s3`, `s5`) that must move to other-side positions.
// Pins the native/wasm `taida_hashmap_resize` side-index rebuild
// during `taida_hashmap_set` walks, AND the fresh-result retain-then-
// push algorithm, on the same code path.
a <= hashMap().set("s1", 1).set("s2", 2).set("s3", 3).set("s4", 4).set("s5", 5).set("s6", 6).set("s7", 7)
b <= hashMap().set("o1", 10).set("o2", 20).set("o3", 30).set("s3", 300).set("o4", 40).set("s5", 500).set("o5", 50).set("o6", 60).set("o7", 70).set("o8", 80).set("o9", 90)
stdout(Str[a.merge(b)]())
