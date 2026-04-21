// C20-2 (ROOT-7): `stdinLine` must return `Lax[Str].failure("")` on
// EOF across all three backends. The Async wrapper resolves immediately
// on Interpreter / Native and via `node:readline/promises`'s `close`
// event on JS; the inner Lax reports `hasValue=false` so callers can
// branch on `.isEmpty()` or fall back with `.getOrDefault("")`.
//
// Driver: run with stdin closed (no input piped).

stdinLine("") ]=> line
stdout("hasValue=" + line.hasValue().toString())
stdout("val=" + line.getOrDefault("<empty>"))
