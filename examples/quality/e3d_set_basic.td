// QF-40: Set basic operations — 3-backend parity test
// Tests: setOf, add, remove, has, size, is_empty, union, intersect, diff, toList, toString

// Create Set
s <= setOf(@[1, 2, 3])
stdout(s.size().toString())
stdout(s.has(2).toString())
stdout(s.has(5).toString())
stdout(s.isEmpty().toString())

// add
s2 <= s.add(4)
stdout(s2.size().toString())

// add duplicate (should not increase size)
s3 <= s.add(2)
stdout(s3.size().toString())

// remove
s4 <= s.remove(2)
stdout(s4.size().toString())
stdout(s4.has(2).toString())

// union
a <= setOf(@[1, 2, 3])
b <= setOf(@[2, 3, 4])
u <= a.union(b)
stdout(u.size().toString())

// intersect
i <= a.intersect(b)
stdout(i.size().toString())

// diff
d <= a.diff(b)
stdout(d.size().toString())

// toList
l <= a.toList()
stdout(l.length().toString())

// empty set
e <= setOf(@[])
stdout(e.isEmpty().toString())
stdout(e.size().toString())

// toString
stdout(a.toString())
