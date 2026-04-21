// C20-2 (ROOT-7 piped case): `stdinLine` must preserve UTF-8 multibyte
// codepoints even when stdin is a pipe / redirect. The interactive
// TTY editing (Backspace on CJK characters) is validated in the
// Hachikuma Phase 11 manual smoke — this fixture covers the CI-
// reachable byte-for-byte identity.
//
// Driver: pipe `こんにちは\n` (15 UTF-8 bytes) into stdin.

stdinLine("") ]=> line
stdout("hasValue=" + line.hasValue().toString())
stdout("val=" + line.getOrDefault("<empty>"))
