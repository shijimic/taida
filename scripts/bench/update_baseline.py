#!/usr/bin/env python3
# C26B-004 / D28B-005 / D28B-013: Update the perf baseline JSON
# (`perf_baseline.json` for throughput, `peak_rss_baseline.json`
# for peak-RSS) with a new sample from a main-branch bench run.
#
# The baseline stores a running median computed as an EWMA with
# `alpha = 1 / min(sample_count + 1, --max-alpha-window)` (default
# alpha-window = 10) and increments `sample_count` each run.
#
# D28B-027 terminology note (Round 2 wH follow-up): the alpha-
# window above (default 10) is distinct from the
# `min_samples_required` field in the baseline JSON (default 30),
# which is the gating threshold at which `compare_baseline.py`
# switches a bench from WARN to hard-fail. The earlier "30-sample
# EWMA window" phrasing in this file conflated the two; the
# precise terminology is "30-sample gating threshold + 10-sample
# alpha-window".
#
# This keeps the baseline drift-resistant (heavy samples converge
# slowly, transient spikes fade) without retaining per-sample history
# (which would inflate the tracked JSON indefinitely).
#
# When a bench is not yet in the baseline, it is added with the
# current sample as `ns_median` and `sample_count=1`.
#
# Usage:
#   update_baseline.py --bencher-out bench-results.txt \
#                      --baseline .github/bench-baselines/perf_baseline.json

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

BENCHER_RE = re.compile(
    r"^test\s+(?P<name>\S+)\s+\.\.\.\s+bench:\s+(?P<ns>[0-9,]+)\s+ns/iter"
)


def parse_bencher(path: Path) -> dict[str, float]:
    results: dict[str, float] = {}
    if not path.exists():
        return results
    for raw in path.read_text().splitlines():
        m = BENCHER_RE.match(raw.strip())
        if not m:
            continue
        results[m.group("name")] = float(m.group("ns").replace(",", ""))
    return results


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--bencher-out", required=True)
    ap.add_argument("--baseline", required=True)
    ap.add_argument(
        "--max-alpha-window",
        type=int,
        default=10,
        help="EWMA converges with alpha = 1/min(sample_count+1, window). "
        "Larger window => slower drift.",
    )
    args = ap.parse_args()

    bencher_path = Path(args.bencher_out)
    baseline_path = Path(args.baseline)

    results = parse_bencher(bencher_path)
    if not results:
        print(f"::warning::no bench results in {bencher_path}; skipping update")
        return 0

    if baseline_path.exists():
        doc = json.loads(baseline_path.read_text())
    else:
        doc = {
            "schema_version": 1,
            "min_samples_required": 30,
            "tolerance_pct": 10.0,
            "benches": {},
        }
    benches = doc.setdefault("benches", {})

    added = 0
    updated = 0
    for name, ns in results.items():
        entry = benches.get(name)
        if entry is None:
            benches[name] = {
                "ns_median": ns,
                "sample_count": 1,
                "notes": "added by scripts/bench/update_baseline.py",
            }
            added += 1
            continue
        sc = int(entry.get("sample_count", 0))
        old_median = float(entry.get("ns_median", ns))
        alpha = 1.0 / min(sc + 1, args.max_alpha_window)
        new_median = (1.0 - alpha) * old_median + alpha * ns
        entry["ns_median"] = new_median
        entry["sample_count"] = sc + 1
        updated += 1

    baseline_path.write_text(json.dumps(doc, indent=2, sort_keys=True) + "\n")
    print(
        f"baseline updated: +{added} new benches, "
        f"{updated} existing benches advanced"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
