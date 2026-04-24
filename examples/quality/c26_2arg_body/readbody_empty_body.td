// C26B-023: readBody on GET request (no body) — 3-backend parity.
//
// When there is no body, readBody should return empty Bytes (length 0).
//
// Expected output (3-backend identical):
//   0

>>> taida-lang/net => @(readBody)

rawLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
rawLax ]=> raw
req <= @(raw <= raw, body <= @(start <= 35, len <= 0))
body <= readBody(req)
stdout(body.length())
