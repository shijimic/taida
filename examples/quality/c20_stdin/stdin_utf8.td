// C20-3 (ROOT-10): JS previously decoded stdin one byte at a time
// with `Buffer.toString('utf-8', 0, 1)`, which corrupts every
// continuation byte into U+FFFD. After the fix all three backends
// read the whole line as a proper UTF-8 string.
//
// Driver pipes "こんにちは\n" (15 bytes, 5 codepoints).

line <= stdin()
// Avoid `length()` here because that method disagrees between the
// three backends for multibyte strings (Interpreter / JS return
// codepoints, Native returns bytes — a pre-existing parity gap
// tracked separately from C20). The C20 contract under test is:
// the returned Str equals the pristine UTF-8 payload, with no
// U+FFFD corruption on any backend.
stdout("got=" + line)
stdout("echo=" + line + "!")
