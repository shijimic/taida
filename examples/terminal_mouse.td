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

>>> taida-lang/terminal => @(mouseTrackingEnter, mouseTrackingLeave, clearScreen, cursorHide, cursorShow, MouseKind, EventKind)

enter_seq <= mouseTrackingEnter()
leave_seq <= mouseTrackingLeave()
clear_seq <= clearScreen()
hide_seq <= cursorHide()
show_seq <= cursorShow()

stdout("mouseTrackingEnter bytes=" + enter_seq.length().toString())
stdout("mouseTrackingLeave bytes=" + leave_seq.length().toString())
stdout("clearScreen bytes=" + clear_seq.length().toString())
stdout("cursorHide bytes=" + hide_seq.length().toString())
stdout("cursorShow bytes=" + show_seq.length().toString())

// Mouse / event kind constants are pinned by the v1 ABI.
stdout("MouseKind.down=" + MouseKind.down.toString())
stdout("MouseKind.scroll_up=" + MouseKind.scroll_up.toString())
stdout("EventKind.mouse=" + EventKind.mouse.toString())
stdout("EventKind.resize=" + EventKind.resize.toString())
