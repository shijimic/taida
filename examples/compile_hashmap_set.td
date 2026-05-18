// === Taida Lang: Native — HashMap & Set ===

// === HashMap ===
m <= hashMap()
stdout("Empty HashMap: " + m.toString())
stdout("isEmpty: " + m.isEmpty().toString())

// set / get
m2 <= m.set("name", "Asuka").set("age", "14")
stdout("m2.get('name') = " + m2.get("name").getOrDefault(""))
stdout("m2.get('age') = " + m2.get("age").getOrDefault(""))
stdout("m2.get('missing') = " + m2.get("missing").getOrDefault("default"))

// has / size
stdout("m2.has('name') = " + m2.has("name").toString())
stdout("m2.size() = " + m2.size().toString())

// keys / values
stdout("m2.keys().length() = " + m2.keys().length().toString())
stdout("m2.values().length() = " + m2.values().length().toString())

// remove
m3 <= m2.remove("age")
stdout("After remove('age'): size = " + m3.size().toString())

// merge
other <= hashMap().set("age", "25").set("city", "Tokyo")
merged <= m2.merge(other)
stdout("After merge: size = " + merged.size().toString())
stdout("Merged age = " + merged.get("age").getOrDefault(""))

// entries
stdout("entries count = " + merged.entries().length().toString())

// === Set ===
stdout("")
stdout("=== Set ===")

// setOf() はインポート不要 (プレリュード)
s <= setOf(@[1, 2, 3, 2, 1])
stdout("setOf(@[1,2,3,2,1]).size() = " + s.size().toString())

// add / remove / has
s2 <= s.add(4)
stdout("After add(4): size = " + s2.size().toString())
stdout("s2.has(4) = " + s2.has(4).toString())

s3 <= s2.remove(1)
stdout("After remove(1): size = " + s3.size().toString())

// union / intersect / diff
a <= setOf(@[1, 2, 3])
b <= setOf(@[2, 3, 4])
stdout("union size = " + a.union(b).size().toString())
stdout("intersect size = " + a.intersect(b).size().toString())
stdout("diff size = " + a.diff(b).size().toString())

// toList
stdout("toList length = " + s.toList().length().toString())

// toString
stdout("Set: " + s.toString())
