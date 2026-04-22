// C23B-008 reopen: merging non-empty self with empty other yields self
// in self-order. Pin that the retain-then-push algorithm degenerates
// correctly when other is empty (no keys to exclude, nothing to
// append).
a <= hashMap().set("x", 1).set("y", 2)
b <= hashMap()
stdout(Str[a.merge(b)]())
