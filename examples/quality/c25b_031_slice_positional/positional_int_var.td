// C25B-031: `Slice[s, pos, end]()` must resolve `pos` as an IntVar on
// native / wasm / js, matching the interpreter. Pre-fix, native / wasm /
// js ignored `pos` (treated it as a named-opts object) and returned the
// full string.
s <= "Hello, World!"
pos <= 7
stdout(Slice[s, pos, 12]())
