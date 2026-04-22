// C23B-008 proactive audit: updating an existing key must NOT change
// its insertion order — only first-insertion-wins. The second .set("a", ...)
// is a value update, so the display order remains "a" (first), "b"
// (second), even though the final value for "a" is 99. Both runtimes'
// `taida_hashmap_set` leave the `order_array` untouched when the key
// already exists.
m <= hashMap().set("a", 1).set("b", 2).set("a", 99)
stdout(Str[m]())
