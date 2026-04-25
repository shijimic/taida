// C27B-028: AsyncReject string display must be byte-identical across
// 3 backends. Pre-fix (Option A applied to taida_str_release without
// the freelist capacity check) the native output of the 4th line drops
// the trailing characters and corrupts the previous chars
// ("something went wrong]" → "something went wRTSD").
//
// Root cause: the small-string freelist bucketed by `total` allowed
// pop callers to receive a slot whose actual aligned data area was
// smaller than the requested length (16-byte alignment vs. ≤32 / ≤64
// / ≤128 bucket maxes). With Option A pushing arena slots, the
// mismatch became deterministic; without it the malloc allocator's
// padding masked the problem most of the time.
//
// The fix records the slot's aligned data-area capacity in hdr[1]
// at push time and the alloc side verifies cap ≥ len + 1 before
// reuse. This fixture exercises the smallest (≤32 / ≤64) buckets
// where the mismatch was largest.

a <= AsyncReject["something went wrong"]()
stdout("Rejected: " + a.toString())

b <= AsyncReject["short err"]()
stdout("Short: " + b.toString())

c <= AsyncReject["a slightly longer error message indeed"]()
stdout("Long: " + c.toString())

// Resolve side: Async[fulfilled string] toString must also stay intact.
d <= Async["resolved value"]()
stdout("Resolved: " + d.toString())
