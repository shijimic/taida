// W-4f2: HashMap basic operations test for wasm-min
// Tests size, has, keys, values, remove, merge, entries AND toString/get content

m <= hashMap()
stdout("isEmpty: " + m.isEmpty().toString())

// set / has / size
m2 <= m.set("name", "Asuka").set("age", "14")
stdout("has name: " + m2.has("name").toString())
stdout("size: " + m2.size().toString())

// keys / values
stdout("keys len: " + m2.keys().length().toString())
stdout("values len: " + m2.values().length().toString())

// remove
m3 <= m2.remove("age")
stdout("after remove size: " + m3.size().toString())

// merge
other <= hashMap().set("city", "Tokyo")
merged <= m2.merge(other)
stdout("merged size: " + merged.size().toString())

// entries
stdout("entries len: " + merged.entries().length().toString())

// W-4f2: toString — verifies full HashMap formatting
stdout(m2.toString())

// W-4f2: after remove toString
stdout(m3.toString())
