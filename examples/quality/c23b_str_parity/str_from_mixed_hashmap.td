// The untyped hashMap() constructor keeps mixed runtime values so Str[]
// exercises per-entry rendering instead of assuming one value tag.
m <= hashMap().set("a", 1).set("b", "x").set("c", 2)
stdout(Str[m]())
