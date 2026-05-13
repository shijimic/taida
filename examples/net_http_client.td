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
response <= result.getOrDefault(@(status <= 0, headers <= @[], body <= ""))
failure <= result.errorInfo().getOrDefault(@(type <= "IoError", message <= "request failed", kind <= "other", code <= 0))
report <= (
  | result.has_value |> "status=" + response.status.toString() + " body_len=" + response.body.length().toString()
  | _              |> failure.message
)
stdout(report)
