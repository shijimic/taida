#!/usr/bin/env python3
# C26B-004: Parse throughput lines from RUN_BENCHMARKS=1 NET6 tests
# and emit a bencher-format line the `compare_baseline.py` gate consumes.
#
# The three integration tests covered here are listed in the C26B-004
# acceptance criteria:
#   - test_net6_3b_native_h2_32_request_throughput_benchmark
#   - test_net6_3b_native_h2_64kib_data_benchmark
#   - test_net6_3b_native_h2_32_stream_multiplex_benchmark
#
# Their eprintln!() output uses the forms:
#   NET6-3b-<N> [...description...] ok=<A>/<B> req/s=<R> total=<T>ms | Design gate: ...
#   NET6-3b-3 [...description...] ok=<A>/<B> throughput=<R>MiB/s total_bytes=<B> elapsed=<T>ms | Design gate: ...
#
# We map each test to a synthetic bench name and emit nanoseconds per
# request (1e9 / req/s, or elapsed_ms / ok for byte-throughput logs),
# so the `compare_baseline.py` gate can apply the same 10% regression
# tolerance.
#
# Usage:
#   parse_net_throughput.py --cargo-test-log cargo-test.log \
#       --output bench-results-net.txt
#
# Input is expected to contain the captured `cargo test -- --nocapture`
# output. All three C26B-004 NET throughput benches must be present;
# otherwise the parser exits non-zero so an infrastructure skip cannot
# masquerade as a green hard-fail gate.

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

NET_RPS_RE = re.compile(
    r"^NET6-(?P<slug>[0-9a-z-]+)\s+\[(?P<desc>[^\]]+)\]\s+"
    r"ok=(?P<ok>\d+)/(?P<n>\d+)\s+"
    r"req/s=(?P<rps>[0-9.]+)\s+total=(?P<total>\d+)ms"
)

NET_BYTES_RE = re.compile(
    r"^NET6-(?P<slug>[0-9a-z-]+)\s+\[(?P<desc>[^\]]+)\]\s+"
    r"ok=(?P<ok>\d+)/(?P<n>\d+)\s+"
    r"throughput=(?P<mibps>[0-9.]+)MiB/s\s+"
    r"total_bytes=(?P<bytes>\d+)\s+elapsed=(?P<elapsed>\d+)ms"
)

# slug -> bench name mapping. Names are stable (baseline JSON keys).
SLUG_TO_BENCH = {
    "3b-2": "net6_3b_native_h2_32_request_throughput",
    "3b-3": "net6_3b_native_h2_64kib_data",
    "3b-4": "net6_3b_native_h2_32_stream_multiplex",
}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cargo-test-log", required=True)
    ap.add_argument("--output", required=True)
    args = ap.parse_args()

    log_path = Path(args.cargo_test_log)
    out_path = Path(args.output)

    if not log_path.exists():
        print(f"::error::log not found: {log_path}", file=sys.stderr)
        out_path.write_text("")
        return 1

    lines_out: list[str] = []
    seen: set[str] = set()
    for raw in log_path.read_text(errors="replace").splitlines():
        line = raw.strip()
        m = NET_RPS_RE.search(line)
        if not m:
            m = NET_BYTES_RE.search(line)
            if not m:
                continue
        slug = m.group("slug")
        bench = SLUG_TO_BENCH.get(slug)
        if bench is None:
            continue
        ok = int(m.group("ok"))
        if ok <= 0:
            print(f"::error::{bench}: ok count is zero in line: {line}", file=sys.stderr)
            out_path.write_text("")
            return 1
        if "rps" in m.groupdict() and m.group("rps") is not None:
            rps = float(m.group("rps"))
            if rps <= 0.0:
                print(f"::error::{bench}: req/s <= 0 in line: {line}", file=sys.stderr)
                out_path.write_text("")
                return 1
            ns_per_req = 1_000_000_000.0 / rps
        else:
            elapsed_ms = int(m.group("elapsed"))
            if elapsed_ms <= 0:
                print(f"::error::{bench}: elapsed <= 0 in line: {line}", file=sys.stderr)
                out_path.write_text("")
                return 1
            ns_per_req = elapsed_ms * 1_000_000.0 / ok
        # ns per request
        # bencher format: `test <name> ... bench: <ns> ns/iter (+/- 0)`
        lines_out.append(
            f"test {bench} ... bench:     {int(ns_per_req):>12,} ns/iter (+/- 0)"
        )
        seen.add(bench)

    missing = [bench for bench in SLUG_TO_BENCH.values() if bench not in seen]
    if missing:
        print(
            "::error::missing required NET6-3b benchmark lines: " + ", ".join(missing),
            file=sys.stderr,
        )
        out_path.write_text("")
        return 1

    out_path.write_text("\n".join(lines_out) + ("\n" if lines_out else ""))
    print(f"wrote {len(lines_out)} bencher line(s) -> {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
