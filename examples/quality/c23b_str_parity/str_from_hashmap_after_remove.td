// C23B-008 proactive audit: HashMap insertion order must survive
// .remove() — removing the middle entry should leave "a" and "c" in
// that order (not re-packed into the hole). Both runtimes now null
// out the corresponding `order_array[i]` slot to -1 on remove; the
// display helpers skip those holes while walking the ordinal range.
// `next_ord` is monotonic, never decremented — it remains as an upper
// bound even though hm[2] (length) decreases.
m <= hashMap().set("a", 1).set("b", 2).set("c", 3).remove("b")
stdout(Str[m]())
