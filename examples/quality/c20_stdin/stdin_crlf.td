// C20-3: Both `\n` and `\r\n` line endings must be stripped before
// the value is returned to Taida. All three backends used to do
// this separately; the C20 refactor keeps the contract identical.

line <= stdin()
stdout("len=" + line.length().toString())
stdout("payload=[" + line + "]")
