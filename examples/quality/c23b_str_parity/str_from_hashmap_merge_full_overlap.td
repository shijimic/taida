// C23B-008 reopen: fully overlapping merge — every self key is in
// other. Interpreter output should be the other map in other-order,
// with other's values (self's entries are all removed during the
// retain step). Previous buggy backends preserved self's ordering
// (`a, b, c`) because update-in-place never moves ordinals.
a <= hashMap().set("a", 1).set("b", 2).set("c", 3)
b <= hashMap().set("c", 30).set("a", 10).set("b", 20)
stdout(Str[a.merge(b)]())
