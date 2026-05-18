k <= 1
m: HashMap[Int, Int] <= hashMap().set(k, 2)
stdout(m.get(1).hasValue().toString())
stdout(m.get(1).getOrDefault(99).toString())
