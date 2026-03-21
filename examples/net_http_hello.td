// net_http_hello.td -- Minimal HTTP server using taida-lang/net
//
// Usage:
//   taida examples/net_http_hello.td
//   (In another terminal: curl http://127.0.0.1:8080/)
//
// The server processes 1 request and exits (maxRequests = 1).

>>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= "Hello from taida-lang/net!")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe(8080, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
