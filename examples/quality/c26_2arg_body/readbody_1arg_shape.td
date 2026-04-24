// C26B-023: readBody on synthetic 1-arg-shape req pack.
//
// 1-arg handler's req pack carries a **buffered body span** @(start, len)
// over req.raw. `readBody(req)` should return those bytes directly.
//
// Expected output (3-backend identical):
//   5
//   hello

>>> taida-lang/net => @(readBody)

rawLax <= Bytes["POST /echo HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello"]()
rawLax ]=> raw
// 1-arg-shape: body span over buffered body (start=42, len=5 for "hello")
req <= @(raw <= raw, body <= @(start <= 42, len <= 5))
body <= readBody(req)
stdout(body.length())
bodyStr <= Utf8Decode[body]().getOrDefault("")
stdout(bodyStr)
