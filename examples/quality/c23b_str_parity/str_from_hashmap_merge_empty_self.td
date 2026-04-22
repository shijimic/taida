// C23B-008 reopen: merging an empty self into non-empty other yields
// other in other-order. Pin that the fresh-result path handles the
// zero-self case without accidentally dropping other entries.
a <= hashMap()
b <= hashMap().set("x", 1).set("y", 2)
stdout(Str[a.merge(b)]())
