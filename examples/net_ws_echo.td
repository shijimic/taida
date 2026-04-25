// net_ws_echo.td -- Minimal WebSocket echo server
//
// Usage (Interpreter):
//   taida examples/net_ws_echo.td
//
// Usage (JS):
//   taida build --target js examples/net_ws_echo.td -o /tmp/net_ws.mjs
//   node /tmp/net_ws.mjs
//
// Usage (Native):
//   taida build --target native examples/net_ws_echo.td -o /tmp/net_ws
//   /tmp/net_ws
//
// Connect with any WebSocket client (e.g. websocat ws://127.0.0.1:8082/ws),
// send a text frame, and receive the same payload back. The server processes
// 1 connection and exits (maxRequests = 1).

>>> taida-lang/net => @(httpServe, wsUpgrade, wsSend, wsReceive, wsClose)

handler req writer =
  upgrade <= wsUpgrade(req, writer)
  ws <= upgrade.__value.ws
  msg <= wsReceive(ws)
  wsSend(ws, msg.__value.data)
  wsClose(ws)
=> :Unit

asyncResult <= httpServe(8082, handler, 1)
asyncResult ]=> result
stdout(result.__value.ok.toString())
