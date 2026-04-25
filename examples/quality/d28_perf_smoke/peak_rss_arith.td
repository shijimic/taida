// D28B-013: Peak-RSS smoke fixture — arithmetic-only baseline.
//
// Purpose: provide a minimum-viable peak-RSS sample (no list, no
// string) so a regression that affects the parser / interpreter
// startup footprint shows up here even when the list / string paths
// are unaffected. The peak RSS for this fixture should hover near
// the interpreter's static memory floor.
//
// Expected stdout: a single integer (42), value not load-bearing.

x <= 40
y <= 2
stdout(x + y)
