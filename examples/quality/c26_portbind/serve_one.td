// C26B-003 port-bind race fixture — minimal 3-backend parity smoke.
//
// Port 18080 is in [10000, 32767] (below the Linux kernel's default
// ip_local_port_range.min = 32768), so the kernel will never re-assign
// this port to an ephemeral socket during the bind window. This is the
// invariant pinned by the allocator fix at tests/parity.rs.
//
// Expected stdout across all 3 backends:
//   true
//   1

>>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= "ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe(18080, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
