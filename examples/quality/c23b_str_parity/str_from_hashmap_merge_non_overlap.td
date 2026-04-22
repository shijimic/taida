// C23B-008 reopen sanity: non-overlapping merge yields self entries in
// self-order followed by other entries in other-order. All 4 backends
// must agree — the previous implementation happened to produce this
// same ordering, but we pin it so a future rewrite doesn't regress the
// no-overlap branch.
a <= hashMap().set("a", 1).set("b", 2)
b <= hashMap().set("c", 3).set("d", 4)
stdout(Str[a.merge(b)]())
