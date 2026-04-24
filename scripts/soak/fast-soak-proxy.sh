#!/usr/bin/env bash
# C26B-005 fast-soak proxy.
#
# This is a **proxy** for the 24h NET scatter-gather soak test pinned
# at `.dev/C26_SOAK_RUNBOOK.md`. The real acceptance is the full 24h
# run; this 30-minute helper exists so an operator can get a
# first-order signal on leak / drift regressions during development
# without committing a full day to each iteration. A PASS from this
# proxy does **not** close the C26B-005 acceptance — only a documented
# 24h PASS recorded in `.dev/C26_SOAK_RUNBOOK.md` does.
#
# Usage:
#   ./scripts/soak/fast-soak-proxy.sh [--duration-min N] [--backend interp|native]
#
# The script:
#   1. Builds a release binary.
#   2. Launches `examples/quality/c26_soak_fixture/main.td` (a
#      `httpServe` scatter-gather loop) on port 0.
#   3. Reads the chosen port from the server's stdout.
#   4. Runs `wrk` or a curl loop against the server for N minutes.
#   5. Samples RSS + fd count every 30s into a CSV.
#   6. Extrapolates a 24h projection (linear fit) and flags drift
#      above 10% / hour as a FAIL signal.
#
# The 24h projection is intentionally conservative — it reports "LIKELY
# STABLE" / "DRIFT DETECTED" / "INCONCLUSIVE" verdicts, never PASS.
# Only the human-driven 24h run earns a PASS in the C26B-005 ledger.

set -euo pipefail

DURATION_MIN=30
BACKEND="interp"

while [ $# -gt 0 ]; do
    case "$1" in
        --duration-min) DURATION_MIN="$2"; shift 2 ;;
        --backend) BACKEND="$2"; shift 2 ;;
        -h|--help)
            sed -n '1,30p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "unknown arg: $1" >&2; exit 1 ;;
    esac
done

if [ "${DURATION_MIN}" -gt 180 ]; then
    echo "--duration-min is capped at 180 (3h) inside the proxy; use the full 24h runbook for the real acceptance" >&2
    exit 1
fi

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "${REPO_ROOT}"

OUTDIR="$(mktemp -d -t fastsoak.XXXXXX)"
trap 'echo "logs at ${OUTDIR}"' EXIT

CSV="${OUTDIR}/samples.csv"
LOG="${OUTDIR}/server.log"
PROJECTION="${OUTDIR}/projection.txt"

echo "timestamp_s,rss_kib,fds" > "${CSV}"

echo "==> building release binary (backend=${BACKEND})"
cargo build --release --bin taida

# Fixture: if not already present, write a minimal scatter-gather
# server that loops forever and echoes three chunks per request.
FIXTURE_DIR="${REPO_ROOT}/examples/quality/c26_soak_fixture"
mkdir -p "${FIXTURE_DIR}"
if [ ! -f "${FIXTURE_DIR}/main.td" ]; then
    cat > "${FIXTURE_DIR}/main.td" <<'TAIDA'
// C26B-005 scatter-gather soak fixture. Kept small so it is easy
// to audit — each request triggers a three-chunk writev and a 512 B
// response body which exercises the scatter-gather path without
// dominating the runtime.
port <= 0
handler = (req) =>
  body <= "abc" <= Repeat["x", 509]() <= "\n"
  @(
    status = 200,
    headers = @(),
    body = body,
  )
httpServe(port, handler)
TAIDA
fi

echo "==> launching server on port 0 (log: ${LOG})"
"${REPO_ROOT}/target/release/taida" "${FIXTURE_DIR}/main.td" > "${LOG}" 2>&1 &
SERVER_PID=$!
trap 'kill ${SERVER_PID} 2>/dev/null || true; echo "logs at ${OUTDIR}"' EXIT

# Wait for port announcement.
PORT=""
for _ in $(seq 1 60); do
    if PORT="$(grep -oE 'listening on [^:]+:[0-9]+' "${LOG}" | tail -1 | grep -oE '[0-9]+$')"; then
        [ -n "${PORT}" ] && break
    fi
    sleep 0.5
done
if [ -z "${PORT}" ]; then
    echo "server did not announce a port within 30s" >&2
    cat "${LOG}" >&2
    exit 2
fi
echo "  server listening on 127.0.0.1:${PORT} (pid=${SERVER_PID})"

# Load generator: curl loop. wrk would be faster but is not always
# present; the purpose is steady scatter-gather traffic, not peak
# throughput. Real acceptance lives in wrk / h2load under the
# 24h runbook.
(
    while true; do
        curl -sS "http://127.0.0.1:${PORT}/" > /dev/null || true
    done
) &
LOAD_PID=$!
trap 'kill ${LOAD_PID} ${SERVER_PID} 2>/dev/null || true; echo "logs at ${OUTDIR}"' EXIT

END_TS=$(( $(date +%s) + DURATION_MIN * 60 ))
echo "==> sampling for ${DURATION_MIN} minutes"
while [ "$(date +%s)" -lt "${END_TS}" ]; do
    TS=$(date +%s)
    if RSS=$(awk '/^VmRSS:/ {print $2}' "/proc/${SERVER_PID}/status" 2>/dev/null); then
        FDS=$(ls "/proc/${SERVER_PID}/fd" 2>/dev/null | wc -l)
        echo "${TS},${RSS:-0},${FDS:-0}" >> "${CSV}"
    else
        echo "server disappeared (pid ${SERVER_PID})" >&2
        exit 3
    fi
    sleep 30
done

kill ${LOAD_PID} ${SERVER_PID} 2>/dev/null || true

# Linear projection to 24h. This intentionally avoids any statistics
# package dependency: awk is enough for a "drift > threshold" signal.
awk -v dur_min="${DURATION_MIN}" -F, 'NR > 1 {
    if (NR == 2) { t0 = $1; rss0 = $2; fd0 = $3 }
    tn = $1; rssn = $2; fdn = $3
}
END {
    if (!t0) { print "INCONCLUSIVE: no samples"; exit 0 }
    dt_hours = (tn - t0) / 3600.0
    if (dt_hours <= 0) { print "INCONCLUSIVE: dt too small"; exit 0 }
    rss_rate_per_hour = (rssn - rss0) / dt_hours
    fd_rate_per_hour = (fdn - fd0) / dt_hours
    rss_24h_proj = rss0 + rss_rate_per_hour * 24
    fd_24h_proj = fd0 + fd_rate_per_hour * 24
    drift_pct = (rss_rate_per_hour / rss0) * 100
    printf "fast-soak proxy %d min\n", dur_min
    printf "  RSS start: %d KiB, end: %d KiB, rate: %.1f KiB/h, 24h proj: %.0f KiB (%.1f%%/h)\n", rss0, rssn, rss_rate_per_hour, rss_24h_proj, drift_pct
    printf "  FD  start: %d, end: %d, rate: %.2f/h, 24h proj: %.0f\n", fd0, fdn, fd_rate_per_hour, fd_24h_proj
    if (drift_pct > 10.0 || fd_rate_per_hour > 5.0) {
        print "VERDICT: DRIFT DETECTED (24h soak will almost certainly fail; fix before investing the real 24h run)"
        exit 4
    }
    print "VERDICT: LIKELY STABLE (proxy PASS does not close C26B-005; run the full 24h soak per `.dev/C26_SOAK_RUNBOOK.md`)"
}' "${CSV}" | tee "${PROJECTION}"

echo "logs, samples, and projection at ${OUTDIR}"
