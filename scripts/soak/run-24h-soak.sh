#!/usr/bin/env bash
# D28B-014 (Round 2 wI; D28 Round 2 review follow-up縮約 2026-04-26)
# — sustained NET soak runner for `.dev/D28_SOAK_RUNBOOK.md`.
#
# Script name (`run-24h-soak.sh`) is legacy from the original 24h-as-
# acceptance era. After the D28 Round 2 review (acceptance縮約: 24h →
# 3h sustained, see `.dev/C26_PROGRESS.md` L195 + `.dev/D28_BLOCKERS
# .md::D28B-014`), the **default duration is 3h** (D28B-014 primary
# acceptance). The script still supports any --duration-hr including
# 24 for extended runs.
#
# This script automates the scatter-gather soak run that closes
# D28B-014 (and D28B-006 by absorption). It is deliberately a thin
# wrapper over the procedure documented in
# `.dev/D28_SOAK_RUNBOOK.md § 2`, not a replacement for that runbook.
# The runbook is the source of truth for what counts as PASS; this
# script just removes the boilerplate of launching the server, the
# load generator, and the monitor loop in three coordinated processes
# without losing them when an SSH session ends.
#
# Compared to `fast-soak-proxy.sh`:
#
#   * fast-soak-proxy is hard-capped at 180 minutes (3h), which now
#     coincides exactly with the D28B-014 primary acceptance duration.
#     Use fast-soak-proxy when you want a single 3h smoke that closes
#     the acceptance, or use this script for the same 3h with the
#     stable output directory + PID files + 4-backend dispatch.
#   * This script has no upper duration cap; --duration-hr accepts
#     any positive value (3 default, 24 for extended optional runs).
#     Output goes to a stable `~/soak-logs/d28/<backend>/<date>/`
#     directory; `setsid nohup` makes the run survive terminal
#     disconnect (especially relevant for 24h extended runs).
#   * This script does *not* return PASS/FAIL by itself; the human
#     judgement loop in runbook § 4 reads the monitor.csv after the
#     window closes and writes the REPORT.md (§ 5.2).
#
# Usage:
#
#   ./scripts/soak/run-24h-soak.sh \
#       [--backend interp|js|native|wasm-wasi] \
#       [--duration-hr N] \
#       [--output DIR] \
#       [--fixture PATH] \
#       [--port N]
#
# Defaults:
#   --backend native
#   --duration-hr 3        (D28B-014 primary acceptance — D28縮約後)
#   --output ~/soak-logs/d28/<backend>/<YYYY-MM-DD>/
#   --fixture examples/quality/d28b_014_24h_soak_fixture/server.td
#                          (fixture 名は legacy、3h / 24h 兼用)
#   --port 18100
#
# Backends:
#
#   interp     ./target/release/taida <fixture>
#   js         taida build --target js → node <bin.mjs>
#   native     taida build --target native → <bin>
#   wasm-wasi  taida build --target wasm-wasi → wasmtime run
#              --tcplisten 0.0.0.0:<port> <bin.wasm>
#              (requires wasmtime >= 24, see docs/STABILITY.md § 5.2)
#
# Detached mode (run survives SSH disconnect):
#
#   nohup ./scripts/soak/run-24h-soak.sh --backend native \
#       > ~/soak-logs/d28/native/$(date +%F)/runner.log 2>&1 &
#   disown
#
# Stop early:
#
#   kill $(cat ~/soak-logs/d28/<backend>/<date>/server.pid)
#   kill $(cat ~/soak-logs/d28/<backend>/<date>/loadgen.pid)
#
# See `.dev/D28_SOAK_RUNBOOK.md § 2.1.1` for the runbook chapter that
# this script implements, and § 7.2 for the generated artifact map.

set -euo pipefail

BACKEND="native"
DURATION_HR=3   # D28B-014 primary acceptance (D28 Round 2 review 縮約 2026-04-26)
OUTPUT=""
FIXTURE=""
PORT=18100

while [ $# -gt 0 ]; do
    case "$1" in
        --backend) BACKEND="$2"; shift 2 ;;
        --duration-hr) DURATION_HR="$2"; shift 2 ;;
        --output) OUTPUT="$2"; shift 2 ;;
        --fixture) FIXTURE="$2"; shift 2 ;;
        --port) PORT="$2"; shift 2 ;;
        -h|--help)
            # D28B-028: clamp to the usage block (lines 1-75 after the
            # D28B-014 縮約 header expansion); the previous `1,72p` and
            # earlier `1,63p` were both off after the docstring grew. The
            # actual usage header now ends at line 75 (last `# See ...`
            # line), and `set -euo pipefail` is on line 77, so 1,75p is
            # the right truncation.
            sed -n '1,75p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "unknown arg: $1" >&2; exit 1 ;;
    esac
done

case "${BACKEND}" in
    interp|js|native|wasm-wasi) ;;
    *)
        echo "unknown backend: ${BACKEND} (expected interp|js|native|wasm-wasi)" >&2
        exit 1
        ;;
esac

# Duration sanity. We *do not* hard-cap at 24h here because the runbook
# also documents 3h retry runs (§ 6.1) and >24h runs are useful for
# investigating slow drifts. We do reject obviously broken inputs.
if ! [[ "${DURATION_HR}" =~ ^[0-9]+(\.[0-9]+)?$ ]]; then
    echo "--duration-hr must be a positive number" >&2
    exit 1
fi
DURATION_S=$(awk -v hr="${DURATION_HR}" 'BEGIN { printf "%d", hr * 3600 }')
if [ "${DURATION_S}" -lt 60 ]; then
    echo "--duration-hr must be >= 0.017 (= 60s); use fast-soak-proxy.sh for sub-minute runs" >&2
    exit 1
fi

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "${REPO_ROOT}"

if [ -z "${OUTPUT}" ]; then
    OUTPUT="${HOME}/soak-logs/d28/${BACKEND}/$(date +%F)"
fi
mkdir -p "${OUTPUT}"

if [ -z "${FIXTURE}" ]; then
    FIXTURE="${REPO_ROOT}/examples/quality/d28b_014_24h_soak_fixture/server.td"
fi
if [ ! -f "${FIXTURE}" ]; then
    echo "fixture not found: ${FIXTURE}" >&2
    exit 1
fi

SERVER_LOG="${OUTPUT}/server.log"
SERVER_PID="${OUTPUT}/server.pid"
LOAD_LOG="${OUTPUT}/loadgen.log"
LOAD_PID="${OUTPUT}/loadgen.pid"
MONITOR_CSV="${OUTPUT}/monitor.csv"
RUNNER_LOG="${OUTPUT}/runner.log"
FINAL_REPORT="${OUTPUT}/final_report.txt"
BUILD_LOG="${OUTPUT}/build.log"

log() {
    local ts
    ts=$(date -Iseconds)
    echo "[${ts}] $*" | tee -a "${RUNNER_LOG}"
}

cleanup() {
    # Best-effort cleanup. The PID files survive so an operator can
    # re-kill if a child stuck. Logs are kept verbatim.
    if [ -f "${LOAD_PID}" ]; then
        kill "$(cat "${LOAD_PID}")" 2>/dev/null || true
    fi
    if [ -f "${SERVER_PID}" ]; then
        kill "$(cat "${SERVER_PID}")" 2>/dev/null || true
        # SIGTERM grace then SIGKILL so the process actually leaves.
        sleep 2
        kill -9 "$(cat "${SERVER_PID}")" 2>/dev/null || true
    fi
}
trap cleanup EXIT

log "==> D28B-014 24h soak runner starting"
log "    backend  = ${BACKEND}"
log "    duration = ${DURATION_HR}h (${DURATION_S}s)"
log "    output   = ${OUTPUT}"
log "    fixture  = ${FIXTURE}"
log "    port     = ${PORT}"

# Build phase. We always build the release binary first so the same
# `target/release/taida` drives every backend dispatch path.
log "==> building release binary"
cargo build --release --bin taida 2>&1 | tee -a "${BUILD_LOG}" >/dev/null

# Backend-specific launch.
case "${BACKEND}" in
    interp)
        log "==> launching interpreter server on port ${PORT}"
        # `setsid` puts the process in its own session so a terminal
        # close does not SIGHUP it. nohup is belt-and-braces.
        TAIDA_NET_ANNOUNCE_PORT=1 \
            setsid nohup "${REPO_ROOT}/target/release/taida" "${FIXTURE}" \
            > "${SERVER_LOG}" 2>&1 &
        echo $! > "${SERVER_PID}"
        ;;
    js)
        if ! command -v node >/dev/null 2>&1; then
            log "ERROR: node is required for --backend js but was not found"
            exit 2
        fi
        BUILT_JS="${OUTPUT}/main.mjs"
        log "==> compiling JS artifact (build log: ${BUILD_LOG})"
        if ! "${REPO_ROOT}/target/release/taida" build --target js \
                "${FIXTURE}" -o "${BUILT_JS}" >> "${BUILD_LOG}" 2>&1; then
            log "ERROR: taida build --target js failed"
            tail -20 "${BUILD_LOG}" | tee -a "${RUNNER_LOG}" >&2
            exit 2
        fi
        log "==> launching node runtime on port ${PORT}"
        TAIDA_NET_ANNOUNCE_PORT=1 \
            setsid nohup node "${BUILT_JS}" \
            > "${SERVER_LOG}" 2>&1 &
        echo $! > "${SERVER_PID}"
        ;;
    native)
        BUILT_NATIVE="${OUTPUT}/main.bin"
        log "==> compiling native artifact (build log: ${BUILD_LOG})"
        if ! "${REPO_ROOT}/target/release/taida" build --target native \
                "${FIXTURE}" -o "${BUILT_NATIVE}" >> "${BUILD_LOG}" 2>&1; then
            log "ERROR: taida build --target native failed"
            tail -20 "${BUILD_LOG}" | tee -a "${RUNNER_LOG}" >&2
            exit 2
        fi
        log "==> launching native binary on port ${PORT}"
        # MALLOC_ARENA_MAX=2 reduces glibc thread-arena step jumps in
        # RSS — see runbook § 4.3 native section. The wF arena reset
        # (D28B-012 FIXED) makes this less load-bearing than it was
        # in the C26 era but the env var does not hurt.
        MALLOC_ARENA_MAX=2 TAIDA_NET_ANNOUNCE_PORT=1 \
            setsid nohup "${BUILT_NATIVE}" \
            > "${SERVER_LOG}" 2>&1 &
        echo $! > "${SERVER_PID}"
        ;;
    wasm-wasi)
        if ! command -v wasmtime >/dev/null 2>&1; then
            log "ERROR: wasmtime is required for --backend wasm-wasi but was not found"
            log "       see docs/STABILITY.md § 5.2 for the version pin (>= 24)"
            exit 2
        fi
        BUILT_WASM="${OUTPUT}/main.wasm"
        log "==> compiling wasm-wasi artifact (build log: ${BUILD_LOG})"
        if ! "${REPO_ROOT}/target/release/taida" build --target wasm-wasi \
                "${FIXTURE}" -o "${BUILT_WASM}" >> "${BUILD_LOG}" 2>&1; then
            log "ERROR: taida build --target wasm-wasi failed"
            tail -20 "${BUILD_LOG}" | tee -a "${RUNNER_LOG}" >&2
            exit 2
        fi
        log "==> launching wasmtime serve on port ${PORT}"
        # wasmtime preopen TCP listener. The exact CLI surface for
        # this is documented at `docs/STABILITY.md § 5.2`; if it
        # changes the runbook references the wasmtime version pin.
        # D28B-028: MALLOC_ARENA_MAX / TAIDA_NET_ANNOUNCE_PORT env-vars
        # are intentionally NOT set for the wasm-wasi backend — wasmtime
        # has its own allocator and the announce mechanism is
        # interpreter / native / JS only. Runbook § 4.3 documents this
        # asymmetry; the launch line below mirrors that documented
        # behaviour.
        setsid nohup wasmtime run --tcplisten "0.0.0.0:${PORT}" "${BUILT_WASM}" \
            > "${SERVER_LOG}" 2>&1 &
        echo $! > "${SERVER_PID}"
        ;;
esac

PID=$(cat "${SERVER_PID}")
log "==> server pid ${PID}, waiting for bind on 127.0.0.1:${PORT}"

# Wait for bind. Up to 60s — wasm-wasi cold start is slower than
# native or interp.
READY=0
for _ in $(seq 1 240); do
    if (exec 3<>/dev/tcp/127.0.0.1/${PORT}) 2>/dev/null; then
        exec 3>&- 3<&-
        READY=1
        break
    fi
    if ! kill -0 "${PID}" 2>/dev/null; then
        log "ERROR: server exited before bind (pid ${PID})"
        tail -50 "${SERVER_LOG}" | tee -a "${RUNNER_LOG}" >&2
        exit 2
    fi
    sleep 0.25
done
if [ "${READY}" -ne 1 ]; then
    log "ERROR: server did not bind 127.0.0.1:${PORT} within 60s"
    tail -50 "${SERVER_LOG}" | tee -a "${RUNNER_LOG}" >&2
    exit 2
fi
log "    server ready on 127.0.0.1:${PORT}"

# Load generator. We use curl in a tight keep-alive loop because it is
# universally available; the runbook documents wrk / h2load as the
# preferred load gens for the manual procedure (§ 2.1.2). The signal
# we care about is sustained traffic, not peak rps.
log "==> launching curl load generator (log: ${LOAD_LOG})"
# D28B-028: write a one-line header so an operator inspecting an empty
# loadgen.log knows the runner started. The previous version sent
# stdout to /dev/null and only appended stderr; on a healthy run the
# log was completely empty, which looked like a runner crash.
echo "# loadgen started $(date -Iseconds) — curl keep-alive loop, http://127.0.0.1:${PORT}/" > "${LOAD_LOG}"
echo "# format: <iso-timestamp>,<http_code>" >> "${LOAD_LOG}"
(
    while kill -0 "${PID}" 2>/dev/null; do
        # `--max-time 5` means a stuck connection cannot block the
        # loop indefinitely; that matters at the 24h tail when the
        # server may be in the middle of a cleanup path.
        # D28B-028: capture HTTP code via --write-out so loadgen.log
        # shows liveness; only sample once per second to keep the log
        # bounded over 24h (~86k lines max).
        ts=$(date -Iseconds)
        code=$(curl -sS --max-time 5 "http://127.0.0.1:${PORT}/" \
                    --output /dev/null --write-out '%{http_code}' \
                    2>>"${LOAD_LOG}" || echo "000")
        echo "${ts},${code}" >> "${LOAD_LOG}"
        sleep 1
    done
) &
echo $! > "${LOAD_PID}"
LOADER_PID=$(cat "${LOAD_PID}")
log "    load gen pid ${LOADER_PID}"

# Monitor loop, 30s interval, runbook-compatible CSV schema.
log "==> launching monitor (csv: ${MONITOR_CSV})"
echo "iso,rss_kb,vsz_kb,fd_count,thread_count,user_cpu_s,sys_cpu_s" > "${MONITOR_CSV}"

START_TS=$(date +%s)
END_TS=$(( START_TS + DURATION_S ))

while [ "$(date +%s)" -lt "${END_TS}" ]; do
    if ! kill -0 "${PID}" 2>/dev/null; then
        log "ERROR: server pid ${PID} disappeared mid-soak"
        tail -100 "${SERVER_LOG}" | tee -a "${RUNNER_LOG}" >&2
        exit 3
    fi
    ts=$(date -Iseconds)
    rss=$(awk '/VmRSS/ {print $2}' "/proc/${PID}/status" 2>/dev/null || echo 0)
    vsz=$(awk '/VmSize/ {print $2}' "/proc/${PID}/status" 2>/dev/null || echo 0)
    fd=$(ls "/proc/${PID}/fd" 2>/dev/null | wc -l)
    thr=$(awk '/Threads/ {print $2}' "/proc/${PID}/status" 2>/dev/null || echo 0)
    read -r utime stime < <(awk '{print $14, $15}' "/proc/${PID}/stat" 2>/dev/null || echo "0 0")
    ucpu=$(awk -v t="${utime}" 'BEGIN { printf "%.2f", t/100 }')
    scpu=$(awk -v t="${stime}" 'BEGIN { printf "%.2f", t/100 }')
    echo "${ts},${rss},${vsz},${fd},${thr},${ucpu},${scpu}" >> "${MONITOR_CSV}"
    sleep 30
done

log "==> soak window closed (${DURATION_HR}h elapsed)"

# Final report. Mirrors the analyze.sh template in runbook § 4.4 but
# inlined so an operator does not have to remember a second script.
{
    echo "D28B-014 24h soak runner final report"
    echo "  backend  = ${BACKEND}"
    echo "  duration = ${DURATION_HR}h"
    echo "  output   = ${OUTPUT}"
    echo "  fixture  = ${FIXTURE}"
    echo "  port     = ${PORT}"
    echo "  start    = $(date -d @${START_TS} -Iseconds)"
    echo "  end      = $(date -d @${END_TS} -Iseconds)"
    echo ""
    awk -F, 'NR > 1 {
        # D28B-028: stime0 was assigned $6 (a typo for $7) — utime0 / stime0 are
        # not consumed in the END block today (CPU drift is not yet a verdict
        # axis), but the typo would silently produce wrong values if a future
        # CPU-drift check is added. Fixed to $7.
        if (NR == 2) { t0=$1; rss0=$2; fd0=$4; thr0=$5; utime0=$6; stime0=$7 }
        tn=$1; rssn=$2; fdn=$4; thrn=$5; utimen=$6; stimen=$7
        n=NR-1
    }
    END {
        if (n < 1) { print "INCONCLUSIVE: no samples"; exit 0 }
        printf "samples: %d\n", n
        printf "RSS: start=%d KiB, end=%d KiB, ratio=%.3f\n", rss0, rssn, (rss0 > 0 ? rssn/rss0 : 0)
        printf "FD:  start=%d, end=%d, delta=%d\n", fd0, fdn, fdn-fd0
        printf "THR: start=%d, end=%d, delta=%d\n", thr0, thrn, thrn-thr0
        # Backend-specific RSS tolerance: native / interp = 1.30,
        # JS / wasm-wasi = 1.40. The runner does not know which it
        # ran (this awk is generic), so it just prints both bands.
        rss_ok_strict = (rss0 > 0 && rssn/rss0 <= 1.30) ? "OK" : "NG (strict 1.30)"
        rss_ok_lax    = (rss0 > 0 && rssn/rss0 <= 1.40) ? "OK" : "NG (lax 1.40)"
        fd_ok = (fdn-fd0 <= 16) ? "OK" : "NG"
        thr_ok = (thrn-thr0 <= 4) ? "OK" : "NG"
        printf "VERDICT (interp/native): RSS %s, FD %s, threads %s\n", rss_ok_strict, fd_ok, thr_ok
        printf "VERDICT (js/wasm-wasi):  RSS %s, FD %s, threads %s\n", rss_ok_lax, fd_ok, thr_ok
        print ""
        print "Read the runbook (.dev/D28_SOAK_RUNBOOK.md § 4) for"
        print "the full PASS/FAIL contract; this report is the inputs"
        print "the human judgement loop uses, not the verdict itself."
    }' "${MONITOR_CSV}"
} > "${FINAL_REPORT}"

cat "${FINAL_REPORT}" | tee -a "${RUNNER_LOG}"

log "==> soak runner finished. artifacts at ${OUTPUT}"
