// compile_list_molds.td -- Native list operation molds test
// Tests: Concat, Append, Prepend, Join, Sum, Sort, Unique,
//        Flatten, FindIndex, Count

// -- Join (test first, used by other tests for output)
words <= @["hello", "world", "taida"]
joined <= Join[words, ", "]()
stdout(joined)

// -- Sum
nums <= @[1, 2, 3, 4, 5]
total <= Sum[nums]()
stdout(total.toString())

// -- Concat
a <= @[1, 2, 3]
b <= @[4, 5, 6]
merged <= Concat[a, b]()
stdout(Sum[merged]().toString())

// -- Append
base1 <= @[10, 20]
c <= Append[base1, 30]()
stdout(Sum[c]().toString())

// -- Prepend
base2 <= @[20, 30]
d <= Prepend[base2, 10]()
stdout(Sum[d]().toString())

// -- Sort (ascending)
unsorted <= @[3, 1, 4, 1, 5, 9]
sorted <= Sort[unsorted]()
sorted_str <= Join[sorted, " "]()
stdout(sorted_str)

// -- Sort (descending with reverse)
sorted_desc <= Sort[unsorted](reverse <= true)
sorted_desc_str <= Join[sorted_desc, " "]()
stdout(sorted_desc_str)

// -- Unique
dupes <= @[1, 2, 2, 3, 3, 3]
uniq <= Unique[dupes]()
stdout(Sum[uniq]().toString())

// -- Flatten
inner1 <= @[1, 2]
inner2 <= @[3, 4]
inner3 <= @[5]
nested <= @[inner1, inner2, inner3]
flat <= Flatten[nested]()
stdout(Sum[flat]().toString())

// -- FindIndex
isEven x: Int =
  Mod[x, 2]() >=> r
  r == 0
=> :Bool
idx <= FindIndex[@[1, 3, 4, 7], isEven]()
stdout(idx.toString())

// -- Count
evens_src <= @[1, 2, 3, 4, 5, 6]
cnt <= Count[evens_src, isEven]()
stdout(cnt.toString())
