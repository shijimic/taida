// C23B-008 reopen repro: HashMap.merge() must move overlapping keys to
// other's ordinal position with other's value (see
// src/interpreter/methods.rs:787-822 — `retain(k != other_key) ; push(other_entry)`).
// Self `a = [a, b]`, other `b3 = [c, b, d]`. Interpreter emits
// `[a, c, b, d]` (b moved to other-side ordinal 2, retains value 20);
// JS / Native / WASM previously did update-in-place and emitted
// `[a, b, c, d]` (b stays at self-side ordinal 1). Fix: fresh map,
// fill with (self\other) then other.
a <= hashMap()
a1 <= a.set("a", 1)
a2 <= a1.set("b", 2)
b <= hashMap()
b1 <= b.set("c", 3)
b2 <= b1.set("b", 20)
b3 <= b2.set("d", 4)
stdout(Str[a2.merge(b3)]())
