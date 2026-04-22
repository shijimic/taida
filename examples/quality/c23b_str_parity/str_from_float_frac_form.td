// C23 parity guard: fractional Float -> Str round-trips the shortest
// decimal form exactly (`"3.14"` / `"-0.5"`) across all 4 backends.
stdout(Str[3.14]())
stdout(Str[-0.5]())
