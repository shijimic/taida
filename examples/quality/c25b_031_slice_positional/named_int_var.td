// C25B-031: named-args form must continue to work (regression guard for
// the pre-existing path — the bug was only in positional form).
s <= "Hello, World!"
pos <= 7
stdout(Slice[s](start <= pos, end <= 12))
