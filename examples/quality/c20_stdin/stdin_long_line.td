// C20-3 (ROOT-8): Native used to read stdin into a 4 KiB stack
// buffer; anything longer than 4095 bytes was truncated and the
// tail bled into the next read, breaking 3-backend parity. After
// the fix Native uses `getline` (POSIX) / realloc loop (Windows),
// matching the dynamic buffers used by Interpreter and JS.
//
// Driver pipes a single ~16 KiB line followed by `\n`. The Taida
// script echoes the length plus the first / last characters of the
// received string so parity tests can pin both the full-length and
// byte-boundary correctness without rendering the raw payload.

line <= stdin()
stdout("len=" + line.length().toString())
stdout("first=" + line.get(0).getOrDefault(""))
stdout("last=" + line.get(line.length() - 1).getOrDefault(""))
