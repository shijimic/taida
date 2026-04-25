// C27B-018: exercise the small-string freelist heavy-traffic path.
// Repeat[ch, n]() allocates many same-sized strings; the freelist
// should recycle slots without size-mismatch corruption.

s1 <= Repeat["x", 31]()
stdout("len 31: [" + s1 + "]")
s2 <= Repeat["y", 47]()
stdout("len 47: [" + s2 + "]")
s3 <= Repeat["z", 31]()
stdout("len 31 again: [" + s3 + "]")
s4 <= Repeat["w", 15]()
stdout("len 15: [" + s4 + "]")
s5 <= Repeat["v", 47]()
stdout("len 47 again: [" + s5 + "]")
