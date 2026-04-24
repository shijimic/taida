#!/usr/bin/env python3
"""C26B-024 Step 1 (wη Round 11) perf-router gate.

Parses `PERF_ROUTER_*` lines emitted by
`tests/c26b_024_router_bench_parity.rs::c26b_024_router_perf_gate`,
applies the thresholds from `.github/bench-baselines/perf_router.json`,
and exits with the appropriate status:

- exit 0: no violations, or violations in warn-only mode
- exit 1: violations in strict mode
- exit 2: malformed/missing measurement input

Warn-only mode is active when `state.sample_count < min_samples_required`
(unless `state.strict` is forced true/false).

Usage:
    python3 scripts/bench/perf_router_gate.py \\
        --test-log cargo-test-perf-router.log \\
        --baseline .github/bench-baselines/perf_router.json \\
        [--update-baseline]   # only main-branch runs pass this flag

Scope isolation: this script is single-purpose and never modifies
`.github/bench-baselines/perf_baseline.json` (owned by `bench.yml` /
C26B-004). It only reads/writes `perf_router.json`.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
import re
import sys
from pathlib import Path
from typing import Dict, Optional


LINE_RE = re.compile(r"^PERF_ROUTER_([A-Z_]+)=([\-0-9\.einfE+]+)\s*$")


def parse_log(path: Path) -> Dict[str, float]:
    """Extract PERF_ROUTER_* key/value pairs from a test log.

    The test emits lines via `println!`; cargo test captures them to
    stdout, so they appear unadorned in the log when `--nocapture` is
    passed. We also accept lines with leading whitespace in case the
    CI shell wraps them.
    """
    found: Dict[str, float] = {}
    with path.open("r", encoding="utf-8", errors="replace") as f:
        for raw in f:
            line = raw.strip()
            m = LINE_RE.match(line)
            if not m:
                continue
            key = m.group(1)
            try:
                found[key] = float(m.group(2))
            except ValueError:
                # NaN / Inf emitted when a backend was skipped; keep it
                # so the caller can decide.
                found[key] = float("nan")
    return found


def ewma(prev: float, new: float, alpha: float = 0.2) -> float:
    """Exponentially-weighted median update.

    alpha = 0.2 weighs each new sample at 20%; this is the same
    constant used by `update_baseline.py` for `perf_baseline.json`
    so both gates have comparable sensitivity.
    """
    if prev <= 0.0:
        return new
    return alpha * new + (1.0 - alpha) * prev


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--test-log", required=True, type=Path)
    ap.add_argument("--baseline", required=True, type=Path)
    ap.add_argument(
        "--update-baseline",
        action="store_true",
        help="On successful runs, increment sample_count and EWMA the medians.",
    )
    ap.add_argument(
        "--sha",
        default="unknown",
        help="Commit SHA recorded in state.last_run_sha (when --update-baseline).",
    )
    args = ap.parse_args()

    if not args.test_log.exists():
        print(f"ERROR: test log not found: {args.test_log}", file=sys.stderr)
        return 2
    if not args.baseline.exists():
        print(f"ERROR: baseline not found: {args.baseline}", file=sys.stderr)
        return 2

    measured = parse_log(args.test_log)
    needed = [
        "NATIVE_JS_RATIO",
        "NATIVE_SYS_RATIO",
        "NATIVE_REAL_MEDIAN_SEC",
        "JS_REAL_MEDIAN_SEC",
    ]
    missing = [k for k in needed if k not in measured]
    if missing:
        print(
            "ERROR: no PERF_ROUTER_* lines found (test likely skipped — "
            "TAIDA_PERF_ROUTER_ENABLED not set, or cc/node unavailable): "
            f"missing={missing}",
            file=sys.stderr,
        )
        # Enforce the workflow's presence assertion here so a silently
        # skipped perf test cannot masquerade as a green gate.
        return 2

    with args.baseline.open("r", encoding="utf-8") as f:
        baseline = json.load(f)

    thresholds = baseline["thresholds"]
    njs_max = float(thresholds["native_js_ratio_max"]["value"])
    sys_max = float(thresholds["sys_real_ratio_max"]["value"])

    state = baseline.get("state", {})
    sample_count = int(state.get("sample_count", 0))
    min_samples = int(baseline.get("min_samples_required", 5))
    strict_override = state.get("strict", None)

    if strict_override is True:
        strict_mode = True
    elif strict_override is False:
        strict_mode = False
    else:
        strict_mode = sample_count >= min_samples

    njs = measured["NATIVE_JS_RATIO"]
    sysr = measured["NATIVE_SYS_RATIO"]

    print("== perf-router gate ==")
    print(f"  native_real_median_sec = {measured['NATIVE_REAL_MEDIAN_SEC']:.4f}")
    print(f"  js_real_median_sec     = {measured['JS_REAL_MEDIAN_SEC']:.4f}")
    print(f"  native_js_ratio        = {njs:.3f} (max {njs_max:.3f})")
    print(f"  native_sys_ratio       = {sysr:.3f} (max {sys_max:.3f})")
    print(f"  sample_count           = {sample_count} / {min_samples}")
    print(f"  strict_mode            = {strict_mode}")

    violations = []
    if not (njs == njs):  # NaN
        print("WARN: native_js_ratio is NaN; skipping njs check")
    elif njs > njs_max:
        violations.append(f"Native/JS ratio {njs:.3f} > max {njs_max:.3f}")

    if not (sysr == sysr):
        print("WARN: native_sys_ratio is NaN; skipping sys check")
    elif sysr > sys_max:
        violations.append(f"Native sys/real {sysr:.3f} > max {sys_max:.3f}")

    if violations:
        if strict_mode:
            print("FAIL: perf-router regression (strict mode)")
            for v in violations:
                print(f"  - {v}")
            return 1
        else:
            print(f"WARN: perf-router drift (sampling phase, {sample_count}/{min_samples})")
            for v in violations:
                print(f"  - {v}")
    else:
        print("OK: perf-router within thresholds")

    # Optional baseline update (main-branch only).
    if args.update_baseline:
        state["sample_count"] = sample_count + 1
        state["median_native_js_ratio"] = ewma(
            float(state.get("median_native_js_ratio", 0.0) or 0.0), njs
        )
        state["median_native_sys_ratio"] = ewma(
            float(state.get("median_native_sys_ratio", 0.0) or 0.0), sysr
        )
        state["last_run_sha"] = args.sha
        state["last_run_at"] = _dt.datetime.now(_dt.timezone.utc).strftime(
            "%Y-%m-%dT%H:%M:%SZ"
        )
        baseline["state"] = state
        with args.baseline.open("w", encoding="utf-8") as f:
            json.dump(baseline, f, indent=2, ensure_ascii=False)
            f.write("\n")
        print(f"baseline updated: sample_count -> {state['sample_count']}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
