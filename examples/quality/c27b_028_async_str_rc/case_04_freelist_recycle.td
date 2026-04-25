// C27B-018: confirm that repeated same-size Str alloc/release
// recycles freelist slots (does not grow unbounded). This fixture
// runs ~1000 iterations of `Repeat["a", 32]()` which all map to the
// same small-string bucket; the freelist (cap=32) plus arena chunks
// must not need fresh arena chunks once the freelist warms up.
//
// We can't directly observe RSS from Taida code, but if the freelist
// is genuinely recycling, the program completes in well under a
// second on a typical machine. A pre-fix Option-A run would either
// corrupt output (which the parity assertion catches) or balloon
// arena chunks past their 128/thread cap and slow down considerably.

iter n =
  | n == 0 |> 0
  | _ |>
      s <= Repeat["a", 32]()
      iter(n - 1)
=> :Int

stdout(iter(1000))
