// RC-1m: Deep nested dependency — leaf module D
// Provides a string-wrapping function (uses strings to avoid WASM int/ptr heuristic)

_internal_d x: Str = "D(" + x + ")" => :Str

wrapD x: Str = _internal_d(x) => :Str

<<< @(wrapD)
