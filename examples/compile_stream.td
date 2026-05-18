// compile_stream.td -- Stream[T] mold type test
// Tests: Stream, StreamFrom, Filter, Map, Take, TakeWhile, pipeline, unmold

// helpers
isEven x: Int =
  Mod[x, 2]() >=> r
  r == 0
=> :Bool
double x: Int = x * 2 => :Int
lessThan5 x: Int = x < 5 => :Bool
timesTen x: Int = x * 10 => :Int

// -- single value Stream[T]() constructor
s0 <= Stream[42]()
s0 >=> r0
stdout(Join[r0, " "]())

// -- single value StreamFrom[list]() constructor
s1_src <= @[42]
s1 <= StreamFrom[s1_src]()
s1 >=> r1
stdout(Join[r1, " "]())

// -- StreamFrom[list]() -- stream from list
nums <= @[1, 2, 3, 4, 5]
s2 <= StreamFrom[nums]()
s2 >=> r2
stdout(Join[r2, " "]())

// -- Map[stream, fn]() -- lazy map
s3_src <= @[10, 20, 30]
s3 <= StreamFrom[s3_src]()
mapped <= Map[s3, double]()
mapped >=> r3
stdout(Join[r3, " "]())

// -- Filter[stream, fn]() -- lazy filter
s4_src <= @[1, 2, 3, 4, 5, 6]
s4 <= StreamFrom[s4_src]()
filtered <= Filter[s4, isEven]()
filtered >=> r4
stdout(Join[r4, " "]())

// -- Take[stream, n]() -- lazy take
s5_src <= @[10, 20, 30, 40, 50]
s5 <= StreamFrom[s5_src]()
taken <= Take[s5, 3]()
taken >=> r5
stdout(Join[r5, " "]())

// -- TakeWhile[stream, pred]() -- lazy take while
s6_src <= @[1, 2, 3, 10, 20]
s6 <= StreamFrom[s6_src]()
tw <= TakeWhile[s6, lessThan5]()
tw >=> r6
stdout(Join[r6, " "]())

// -- Chained transforms: Filter => Map => Take
s7_src <= @[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
s7 <= StreamFrom[s7_src]()
step1 <= Filter[s7, isEven]()
step2 <= Map[step1, timesTen]()
step3 <= Take[step2, 3]()
step3 >=> results
stdout(Join[results, " "]())

// -- Stream methods
s8_src <= @[1, 2, 3]
s8 <= StreamFrom[s8_src]()
stdout(s8.toString())
len8 <= s8.length()
stdout(len8.toString())
empty8 <= s8.isEmpty()
stdout(empty8.toString())

// -- Empty stream
empty_src: @[Int] <= @[]
s9 <= StreamFrom[empty_src]()
empty9 <= s9.isEmpty()
stdout(empty9.toString())
