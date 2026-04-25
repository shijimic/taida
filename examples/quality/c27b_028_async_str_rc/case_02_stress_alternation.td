// C27B-028 stress: alternate small / medium AsyncReject strings so
// the freelist receives slots of distinct aligned sizes within the
// same bucket. Verifies the capacity check correctly rejects
// too-small slots instead of corrupting the next allocation.

a1 <= AsyncReject["short"]()
stdout(a1.toString())
b1 <= AsyncReject["a much longer error message that fits in bucket 1"]()
stdout(b1.toString())
c1 <= AsyncReject["mid sized err"]()
stdout(c1.toString())

a2 <= AsyncReject["short"]()
stdout(a2.toString())
b2 <= AsyncReject["a much longer error message that fits in bucket 1"]()
stdout(b2.toString())
c2 <= AsyncReject["mid sized err"]()
stdout(c2.toString())

a3 <= AsyncReject["short"]()
stdout(a3.toString())
b3 <= AsyncReject["a much longer error message that fits in bucket 1"]()
stdout(b3.toString())
c3 <= AsyncReject["mid sized err"]()
stdout(c3.toString())
