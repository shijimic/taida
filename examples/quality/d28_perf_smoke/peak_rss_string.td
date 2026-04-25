// D28B-013: Peak-RSS smoke fixture — string allocation churn.
//
// Purpose: exercise the Str primitive allocation + concat/repeat path
// (Repeat returns :Str) with a 4096-iter expansion so /usr/bin/time
// captures a non-trivial peak RSS value. Kept under 1s wallclock so
// the gate adds < 5s to CI bench runs.
//
// Expected stdout: a single integer (string length), value not
// load-bearing — the gate compares peak_rss in KiB only.

base <= "taida-lang"
big <= Repeat[base, 4096]()
stdout(big.length())
