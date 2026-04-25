// net_http_client.td -- HTTP client using HttpRequest mold (taida-lang/os)
//
// Usage (Interpreter):
//   taida examples/net_http_client.td <url>
//
// Usage (JS):
//   taida build --target js examples/net_http_client.td -o /tmp/net_http_client.mjs
//   node /tmp/net_http_client.mjs <url>
//
// Usage (Native):
//   taida build --target native examples/net_http_client.td -o /tmp/net_http_client
//   /tmp/net_http_client <url>
//
// Sends a single GET request and prints status + body length.

>>> taida-lang/os => @(argv)

args <= argv()
url <= (
  | args.length() > 0 |> args.first().getOrDefault("https://example.com")
  | _ |> "https://example.com"
)

resp <= HttpRequest["GET", url](
  headers <= @[@(name <= "user-agent", value <= "taida-net-example/1.0")],
  body <= ""
)
resp ]=> result
report <= (
  | result.hasValue |> "status=" + result.__value.status.toString() + " body_len=" + result.__value.body.length().toString()
  | _              |> "request failed: " + result.__error.message
)
stdout(report)
