// Taida Lang Example: Async Basics
// Async[T] is a mold type for asynchronous operations.
// In the current interpreter, async is simulated synchronously.

// Create a fulfilled async value
resolved <= Async[42]()
stdout("Resolved async: " + resolved.toString())

// Unwrap with >=> (blocking await)
resolved >=> value
stdout("Unwrapped value: " + value.toString())

// Status check methods
stdout("isFulfilled: " + resolved.isFulfilled().toString())
stdout("isRejected: " + resolved.isRejected().toString())

// Create a rejected async
rejected: Async[Str] <= AsyncReject["something went wrong"]()
stdout("Rejected async: " + rejected.toString())
stdout("isRejected: " + rejected.isRejected().toString())

// getOrDefault — non-blocking fallback
stdout("getOrDefault fulfilled: " + resolved.getOrDefault(0).toString())
stdout("getOrDefault rejected: " + rejected.getOrDefault(99).toString())

// map — transform the inner value
double x: Int =
  x * 2
=> :Int

doubled <= resolved.map(double)
doubled >=> doubledVal
stdout("Mapped (42 * 2): " + doubledVal.toString())

// map on rejected propagates rejection
rejectedMapped <= rejected.map(_ x: Str = x + "!")
stdout("Map on rejected still rejected: " + rejectedMapped.isRejected().toString())

// All — collect all results
asyncs <= @[Async[1](), Async[2](), Async[3]()]
All[asyncs]() >=> allResults
stdout("All results:")
stdout(allResults)

// Race — first resolved value
asyncs2 <= @[Async[10](), Async[20]()]
Race[asyncs2]() >=> winner
stdout("Race winner: " + winner.toString())

// Timeout — passthrough in synchronous mode
Timeout[Async[42](), 5000]() >=> timeoutVal
stdout("Timeout value: " + timeoutVal.toString())

// Error ceiling catches rejected async on unmold
fetchData key: Str =
  |== error: Error =
    "fallback: " + key
  => :Str

  a <= AsyncReject["network error"]()
  a >=> data
  data.toString()
=> :Str

result <= fetchData("config")
stdout("Error ceiling result: " + result)

// Chaining async operations
addTen x: Int =
  Async[x + 10]()
=> :Async[Int]

a <= Async[0]()
a >=> v1
b <= addTen(v1)
b >=> v2
c <= addTen(v2)
c >=> v3
stdout("Chained result (0 + 10 + 10): " + v3.toString())
