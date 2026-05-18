// net_http_hello.td -- Minimal HTTP server using taida-lang/net
//
// Usage (Interpreter):
//   taida examples/net_http_hello.td
//
// Usage (JS):
//   taida build --target js examples/net_http_hello.td -o /tmp/net_hello.mjs
//   node /tmp/net_hello.mjs
//
// Usage (Native):
//   taida build --target native examples/net_http_hello.td -o /tmp/net_hello
//   /tmp/net_hello
//
// Then: curl http://127.0.0.1:8080/
//
// The server processes 1 request and exits (maxRequests = 1).

>>> taida-lang/net => @(httpServe)

handler req: Request =
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= "Hello from taida-lang/net!")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe(8080, handler, 1)
asyncResult >=> result
result >=> r
stdout(r.ok)
stdout(r.requests)
