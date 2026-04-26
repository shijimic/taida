#!/usr/bin/env python3
# Perf regression gate — baseline comparison.
#
# Origin: C26B-004 (throughput gate). D28B-013 reused the same engine
# for the peak-RSS gate; D28B-027 (Round 2 wH follow-up) split out the
# `--gate-id` flag so log lines are not hard-coded to "C26B-004" when
# a different gate ID is the actual caller.
#
# Reads a criterion bencher-format result file (produced by
# `cargo bench ... --output-format bencher`) and compares each
# benchmark against the committed baseline JSON at
# `.github/bench-baselines/perf_baseline.json` (throughput) or
# `scripts/perf/peak_rss_baseline.json` (peak-RSS).
#
# Exit codes:
#   0 — all benchmarks within tolerance (or baseline is still
#       collecting and --require-baseline is not set)
#   1 — at least one benchmark regressed beyond tolerance
#   2 — missing baseline entries and --require-baseline was set
#
# The gate tolerates up to `--tolerance-pct` slow-down (default 10.0).
# Baseline sample count requirement is enforced via
# `--min-samples` (default 30, per C26B-004 acceptance).
#
# Usage:
#   compare_baseline.py \
#       --bencher-out bench-results.txt \
#       --baseline .github/bench-baselines/perf_baseline.json \
#       --tolerance-pct 10.0 \
#       --min-samples 30 \
#       --gate-id D28B-005 \
#       [--require-baseline]
#
# Scope: this script is invoked from .github/workflows/bench.yml.
# It is intentionally standalone (no deps beyond stdlib) so the gate
# can run inside the bench job without pulling Python packages.

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path
from typing import Dict, Optional

BENCHER_RE = re.compile(
    r"^test\s+(?P<name>\S+)\s+\.\.\.\s+bench:\s+(?P<ns>[0-9,]+)\s+ns/iter"
)


def parse_bencher(path: Path) -> Dict[str, float]:
    """Return {bench_name: ns_per_iter} from bencher-format output."""
    results: Dict[str, float] = {}
    for raw in path.read_text().splitlines():
        m = BENCHER_RE.match(raw.strip())
        if not m:
            continue
        name = m.group("name")
        ns = float(m.group("ns").replace(",", ""))
        results[name] = ns
    return results


def load_baseline(path: Path) -> Dict[str, dict]:
    """Return baseline structure keyed by bench name.

    Baseline schema:
      {
        "schema_version": 1,
        "min_samples_required": 30,
        "benches": {
          "<bench_name>": {
            "ns_median": <float>,
            "sample_count": <int>,
            "notes": "..."
          },
          ...
        }
      }
    """
    if not path.exists():
        return {}
    doc = json.loads(path.read_text())
    benches = doc.get("benches", {})
    assert isinstance(benches, dict)
    return benches


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bencher-out", required=True)
    ap.add_argument("--baseline", required=True)
    ap.add_argument("--tolerance-pct", type=float, default=10.0)
    ap.add_argument(
        "--min-samples",
        type=int,
        default=30,
        help="Minimum sample_count required per bench before the gate "
        "actually fails on regression (C26B-004 acceptance).",
    )
    ap.add_argument(
        "--require-baseline",
        action="store_true",
        help="Fail if any bench in the bencher output is missing from "
        "the baseline (used after baseline has stabilised).",
    )
    ap.add_argument(
        "--gate-id",
        type=str,
        default="C26B-004",
        help="Identifier prefixed to log / error lines. Defaults to "
        "C26B-004 for backward compatibility; bench.yml passes "
        "D28B-005 for throughput and D28B-013 for peak-RSS so the "
        "actual gate is identifiable in CI logs (D28B-027).",
    )
    args = ap.parse_args()
    gate = args.gate_id

    bencher_out = Path(args.bencher_out)
    baseline_path = Path(args.baseline)

    if not bencher_out.exists():
        print(
            f"::error::bencher output not found: {bencher_out}",
            file=sys.stderr,
        )
        return 1

    results = parse_bencher(bencher_out)
    if not results:
        print(
            f"::error::no bencher-format lines parsed from {bencher_out}. "
            "Did `cargo bench --output-format bencher` run?",
            file=sys.stderr,
        )
        return 1

    baseline = load_baseline(baseline_path)

    print(f"{gate} regression gate: parsed {len(results)} benches")
    print(
        f"{gate} regression gate: baseline has {len(baseline)} entries "
        f"(schema at {baseline_path})"
    )
    print(f"{gate} regression gate: tolerance = +{args.tolerance_pct:.2f}%")
    print(f"{gate} regression gate: min samples = {args.min_samples}")

    regressions: list[str] = []
    missing: list[str] = []

    for name in sorted(results.keys()):
        ns_current = results[name]
        entry: Optional[dict] = baseline.get(name)
        if entry is None:
            missing.append(name)
            print(
                f"  MISS  {name}: no baseline entry "
                f"(current = {ns_current:.0f} ns/iter)"
            )
            continue
        ns_base = float(entry["ns_median"])
        samples = int(entry.get("sample_count", 0))
        if ns_base <= 0.0:
            # Scaffold entry with no real sample yet. Treat as
            # warn-only regardless of sample_count bookkeeping so a
            # zero baseline cannot short-circuit the gate.
            print(
                f"  WARN  {name}: baseline ns_median=0.0 (pristine), "
                f"current = {ns_current:.0f} ns/iter, samples={samples}"
            )
            continue
        delta_pct = (ns_current - ns_base) / ns_base * 100.0
        status = "OK"
        if samples < args.min_samples:
            status = (
                f"WARN (baseline only has {samples} samples, "
                f"< min {args.min_samples}; regression gate suppressed)"
            )
        elif delta_pct > args.tolerance_pct:
            status = (
                f"FAIL (regressed +{delta_pct:.2f}% > "
                f"+{args.tolerance_pct:.2f}% tolerance)"
            )
            regressions.append(
                f"{name}: +{delta_pct:.2f}% "
                f"({ns_base:.0f} -> {ns_current:.0f} ns/iter)"
            )
        print(
            f"  {status}  {name}: {ns_base:.0f} -> "
            f"{ns_current:.0f} ns/iter ({delta_pct:+.2f}%, "
            f"samples={samples})"
        )

    exit_code = 0
    if regressions:
        print("")
        print(f"::error::{gate} regression gate: hard-fail")
        for line in regressions:
            print(f"  - {line}")
        exit_code = 1

    if args.require_baseline and missing:
        print("")
        print(
            f"::error::{gate} regression gate: missing baseline entries "
            "(require-baseline set)"
        )
        for name in missing:
            print(f"  - {name}")
        if exit_code == 0:
            exit_code = 2

    if exit_code == 0:
        print("")
        print(f"{gate} regression gate: PASS")
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
