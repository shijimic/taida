// C23B-002 + C23B-003 guard: `Str[@(a <= 1)]()` must produce `@(a <= 1)`
// on all 4 backends. JS previously returned `[object Object]` via
// `String(value)`; native / wasm returned raw pointer integers.
stdout(Str[@(a <= 1)]())
