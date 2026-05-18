// net_sse_broadcaster.td -- Server-Sent Events broadcaster
//
// Usage (Interpreter):
//   taida examples/net_sse_broadcaster.td
//
// Usage (JS):
//   taida build --target js examples/net_sse_broadcaster.td -o /tmp/net_sse.mjs
//   node /tmp/net_sse.mjs
//
// Usage (Native):
//   taida build --target native examples/net_sse_broadcaster.td -o /tmp/net_sse
//   /tmp/net_sse
//
// Then: curl -N http://127.0.0.1:8081/events
//
// Demonstrates the streaming form of httpServe (2-arg handler) with sseEvent.
// The server emits 3 events on each connection and ends the response.

>>> taida-lang/net => @(httpServe, sseEvent, endResponse)

handler req: Request writer: Writer =
  sseEvent(writer, "status", "online")
  sseEvent(writer, "tick", "1")
  sseEvent(writer, "tick", "2")
  endResponse(writer)
=> :Int

asyncResult <= httpServe(8081, handler, 1)
asyncResult >=> result
result >=> served
stdout(served.ok.toString())
stdout(served.requests.toString())
