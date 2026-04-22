// C23B-008 repro: multi-entry HashMap's full-form display must iterate
// in insertion order ("a" before "b") regardless of bucket hash layout.
// Native / WASM previously walked buckets linearly, emitting "b" before
// "a" because `taida_str_hash("b") % 16 < taida_str_hash("a") % 16`.
// Fix: HashMap allocation carries an insertion-order side-index
// (`next_ord` + `order_array[cap]`) appended after entries; display /
// iteration helpers walk that side-index. JS was already correct (its
// `__entries` is a plain Array).
m <= hashMap().set("a", 1).set("b", 2)
stdout(Str[m]())
