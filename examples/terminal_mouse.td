// terminal_mouse.td -- Mouse tracking demo using taida-lang/terminal
//
// Usage (Interpreter, requires terminal addon installed):
//   taida install taida-lang/terminal
//   taida examples/terminal_mouse.td
//
// Demonstrates SGR 1006 mouse tracking enable / leave string emission.
// The example renders the ANSI control sequences and prints a
// human-readable byte count so the example works without an actual
// TTY (CI / redirected stdout safe).

>>> taida-lang/terminal => @(MouseTrackingEnter, MouseTrackingLeave, ClearScreen, CursorHide, CursorShow, MouseKind, EventKind)

enterSeq <= MouseTrackingEnter()
leaveSeq <= MouseTrackingLeave()
clearSeq <= ClearScreen()
hideSeq <= CursorHide()
showSeq <= CursorShow()

stdout("MouseTrackingEnter bytes=" + enterSeq.length().toString())
stdout("MouseTrackingLeave bytes=" + leaveSeq.length().toString())
stdout("ClearScreen bytes=" + clearSeq.length().toString())
stdout("CursorHide bytes=" + hideSeq.length().toString())
stdout("CursorShow bytes=" + showSeq.length().toString())

// Mouse / event kind constants are pinned by the v1 ABI.
stdout("MouseKind.LeftDown=" + MouseKind.LeftDown.toString())
stdout("MouseKind.ScrollUp=" + MouseKind.ScrollUp.toString())
stdout("EventKind.Mouse=" + EventKind.Mouse.toString())
stdout("EventKind.Resize=" + EventKind.Resize.toString())
