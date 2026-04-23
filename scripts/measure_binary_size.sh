#!/usr/bin/env bash
# C25B-009: measure `target/release/taida` size + startup time and
# emit a JSON blob matching scripts/binary_size_baseline.json's schema.
#
# Usage:
#   scripts/measure_binary_size.sh                   # print to stdout
#   scripts/measure_binary_size.sh > out.json        # pipe to file
#   scripts/measure_binary_size.sh --refresh-baseline  # overwrite baseline
#
# This script is invoked from .github/workflows/binary_size.yml and is
# safe to run locally to check whether a local change has blown the
# +10% budget (see docs/STABILITY.md §5.5 and .dev/C25_BLOCKERS.md::C25B-009).
#
# Refreshing the baseline should only happen after an explicit
# maintainer approval — run with --refresh-baseline after a clean
# release build on the reference host, commit the JSON, and note
# the capture context (tag, commit, host) in the file header.

set -euo pipefail

BIN="target/release/taida"
if [ ! -x "$BIN" ]; then
  echo "build release binary first: cargo build --release --bin taida" >&2
  exit 1
fi

BYTES=$(stat -c%s "$BIN")
TEXT=$(size "$BIN" | awk 'NR==2 {print $1}')
DATA=$(size "$BIN" | awk 'NR==2 {print $2}')
BSS=$(size "$BIN"  | awk 'NR==2 {print $3}')

# Warm-up three times so the page cache is populated before measuring.
for _ in 1 2 3; do "$BIN" --version > /dev/null; done

STARTUP_MS=$(python3 - "$BIN" <<'PY'
import subprocess, sys, time
bin = sys.argv[1]
N = 20
total = 0.0
for _ in range(N):
    t0 = time.perf_counter_ns()
    subprocess.run([bin, "--version"], stdout=subprocess.DEVNULL, check=True)
    total += (time.perf_counter_ns() - t0)
print("%.6f" % (total / N / 1e6))
PY
)

JSON=$(cat <<EOF
{
  "bytes": $BYTES,
  "text": $TEXT,
  "data": $DATA,
  "bss": $BSS,
  "startup_ms": $STARTUP_MS
}
EOF
)

if [ "${1-}" = "--refresh-baseline" ]; then
  python3 - "$JSON" <<'PY'
import json, os, sys
new = json.loads(sys.argv[1])
path = os.path.join("scripts", "binary_size_baseline.json")
with open(path) as f:
    base = json.load(f)
for k in ("bytes", "text", "data", "bss", "startup_ms"):
    base[k] = new[k]
with open(path, "w") as f:
    json.dump(base, f, indent=2)
    f.write("\n")
print(f"refreshed {path}", file=sys.stderr)
PY
else
  echo "$JSON"
fi
