// C20-3 (ROOT-9): `stdin` must return `""` on EOF across all three
// backends. Interpreter previously threw `IoError`, JS / Native
// silently returned empty. C20 aligns everyone on the empty-string
// fallback so `stdin` remains a default-value-preserving API and
// callers that need failure awareness switch to `stdinLine`.
//
// Driver: run with stdin closed (no input piped).

line <= stdin()
stdout("len=" + line.length().toString())
stdout("ok")
