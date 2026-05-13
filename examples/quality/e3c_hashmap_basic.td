// QF-40: HashMap basic operations — 3-backend parity test
// Tests: hashMap(), set, get, has, size, is_empty, remove, merge, keys, values, entries, toString

// Empty HashMap
m <= hashMap()
stdout(m.size().toString())
stdout(m.isEmpty().toString())

// set and chain
m1 <= m.set("x", 10).set("y", 20)
stdout(m1.size().toString())
stdout(m1.has("x").toString())
stdout(m1.has("z").toString())

// get returns Lax — unmold to get value
m1.get("x") ]=> v
stdout(v.toString())

// get missing key — hasValue is false
zLax <= m1.get("z")
stdout(zLax.has_value.toString())

// remove
m2 <= m1.remove("x")
stdout(m2.size().toString())
stdout(m2.has("x").toString())

// merge
m3 <= hashMap().set("a", 1)
m4 <= hashMap().set("b", 2)
merged <= m3.merge(m4)
stdout(merged.size().toString())
stdout(merged.has("a").toString())
stdout(merged.has("b").toString())

// keys and values
k <= m1.keys()
stdout(k.length().toString())

vals <= m1.values()
stdout(vals.length().toString())

// entries
e <= m1.entries()
stdout(e.length().toString())

// toString
stdout(m3.toString())
