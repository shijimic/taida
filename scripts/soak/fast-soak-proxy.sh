#!/usr/bin/env bash
# C26B-005 / C27B-015 / C27B-017 fast-soak proxy.
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
#   ./scripts/soak/fast-soak-proxy.sh [--duration-min N] [--backend interp|js|native]
#
# Backend dispatch (C27B-015):
#   --backend interp  → target/release/taida <fixture>           (port 18081)
#   --backend js      → taida build --target js --run + node     (port 18082)
#   --backend native  → taida build --target native --run        (port 18083)
#
# Each backend uses a distinct port so all three can be launched in
# parallel from three terminals (or three CI shards) without bind
# contention. CSV / log / projection paths are per-invocation
# tempdirs, so parallel runs do not clobber each other.
#
# A backend's exit code is independent — JS / native build failures
# cannot be hidden by a successful interpreter run, because each
# invocation starts at most one server process.
#
# Optional env vars:
#   TAIDA_NET_ANNOUNCE_PORT=1  Inline-exported to every spawned server
#                              (interp / js / native) by this script
#                              regardless of whether the caller sets
#                              it. Causes `httpServe` to print one
#                              stdout line of the form
#                              `listening on 127.0.0.1:NNNNN` before
#                              the first accept(). The caller can also
#                              set it explicitly to be defensive.
#                              See `.dev/C27_BLOCKERS.md::C27B-014`.
#
#   USE_ANNOUNCE=1             Opt-in: instead of probing the fixed
#                              per-backend PORT via /dev/tcp, parse the
#                              `listening on <host>:<port>` line out of
#                              the server LOG and overwrite PORT with
#                              the announced value. Required for the
#                              port=0 / OS-assigned-port flow used by
#                              the soak runbook. Default OFF (legacy
#                              fixed-port TCP probe is preserved so
#                              existing CI smoke and parallel runs are
#                              unchanged).
#
# CI smoke (C27B-017): `.github/workflows/soak-smoke.yml` runs this
# script with `--duration-min 1 --backend interp`. The job fails
# fast on parse errors, bind failures, and missing VERDICT lines.
#
# The script:
#   1. Builds a release binary.
#   2. Launches `examples/quality/c26_soak_fixture/main.td` (a
#      `httpServe` scatter-gather loop) on the per-backend port.
#   3. Probes the port via /dev/tcp until ready.
#   4. Runs a curl loop against the server for N minutes.
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
            # Print the leading docblock (file header through the end of
            # the descriptive comments). C27B-015 / wA Round 2: the
            # docblock now extends past line 60 because of the multi-
            # backend, TAIDA_NET_ANNOUNCE_PORT inline-export, and
            # USE_ANNOUNCE=1 LOG-readback notes, so widen the slice to
            # the line right above `set -euo pipefail` to cover the
            # full header.
            sed -n '1,72p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "unknown arg: $1" >&2; exit 1 ;;
    esac
done

if [ "${DURATION_MIN}" -gt 180 ]; then
    echo "--duration-min is capped at 180 (3h) inside the proxy; use the full 24h runbook for the real acceptance" >&2
    exit 1
fi

case "${BACKEND}" in
    interp) PORT=18081 ;;
    js)     PORT=18082 ;;
    native) PORT=18083 ;;
    *) echo "unknown backend: ${BACKEND} (expected interp|js|native)" >&2; exit 1 ;;
esac

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "${REPO_ROOT}"

OUTDIR="$(mktemp -d -t fastsoak.XXXXXX)"
trap 'echo "logs at ${OUTDIR}"' EXIT

CSV="${OUTDIR}/samples.csv"
LOG="${OUTDIR}/server.log"
BUILD_LOG="${OUTDIR}/build.log"
PROJECTION="${OUTDIR}/projection.txt"
FIXTURE="${OUTDIR}/main.td"

echo "timestamp_s,rss_kib,fds" > "${CSV}"

echo "==> building release binary (host, backend=${BACKEND})"
cargo build --release --bin taida

# Fixture: regenerated per-invocation inside OUTDIR (so parallel runs
# of different backends do not clobber each other's port). The runtime
# writev (scatter-gather) lives inside httpServe's response path, not
# in user code — the fixture is a steady 1-arg handler returning a
# 512 B body.
#
# Port policy: per-backend fixed port (interp=18081 / js=18082 /
# native=18083), all below Linux ip_local_port_range.min = 32768 so
# the kernel will not reassign them to ephemeral sockets mid-bind.
# Using fixed ports avoids a `getsockname` round-trip the shell proxy
# cannot express today — tracked as C27B-014 for the port-0 flow.
FIXTURE_MAX_REQS=1000000000
cat > "${FIXTURE}" <<TAIDA
// C26B-005 scatter-gather soak fixture. Kept small so it is easy to
// audit — each request returns a 512 B body which exercises the
// runtime's scatter-gather (writev) response path without dominating
// the runtime budget. Regenerated by scripts/soak/fast-soak-proxy.sh.
>>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= Repeat["x", 512]())
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe(${PORT}, handler, ${FIXTURE_MAX_REQS})
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
TAIDA

# Dispatch per backend. All three paths produce a long-running server
# process whose PID is captured in SERVER_PID so the sampling loop can
# read /proc/<pid>/status.
# Forward TAIDA_NET_ANNOUNCE_PORT=1 to every spawned server so the
# bind path emits `listening on 127.0.0.1:NNNNN` on its own stdout
# line (see `.dev/C27_BLOCKERS.md::C27B-014`). The proxy's caller can
# also set this env var explicitly; we re-export it here defensively
# so the 3 launch paths agree even if invoked through `env -i` or a
# CI step that does not propagate the parent env (e.g. a sudo step).
# This is the contract the docblock at the top of this script claims.
case "${BACKEND}" in
    interp)
        echo "==> launching interpreter server on port ${PORT} (log: ${LOG})"
        TAIDA_NET_ANNOUNCE_PORT=1 "${REPO_ROOT}/target/release/taida" "${FIXTURE}" > "${LOG}" 2>&1 &
        SERVER_PID=$!
        ;;
    js)
        if ! command -v node >/dev/null 2>&1; then
            echo "node is required for --backend js but was not found on PATH" >&2
            exit 1
        fi
        BUILT_JS="${OUTDIR}/main.mjs"
        echo "==> compiling JS artifact (log: ${BUILD_LOG})"
        if ! "${REPO_ROOT}/target/release/taida" build --target js "${FIXTURE}" -o "${BUILT_JS}" > "${BUILD_LOG}" 2>&1; then
            echo "taida build --target js failed:" >&2
            cat "${BUILD_LOG}" >&2
            exit 2
        fi
        echo "==> launching node runtime on port ${PORT} (log: ${LOG})"
        TAIDA_NET_ANNOUNCE_PORT=1 node "${BUILT_JS}" > "${LOG}" 2>&1 &
        SERVER_PID=$!
        ;;
    native)
        BUILT_NATIVE="${OUTDIR}/main"
        echo "==> compiling native artifact (log: ${BUILD_LOG})"
        if ! "${REPO_ROOT}/target/release/taida" build --target native "${FIXTURE}" -o "${BUILT_NATIVE}" > "${BUILD_LOG}" 2>&1; then
            echo "taida build --target native failed:" >&2
            cat "${BUILD_LOG}" >&2
            exit 2
        fi
        echo "==> launching native binary on port ${PORT} (log: ${LOG})"
        TAIDA_NET_ANNOUNCE_PORT=1 "${BUILT_NATIVE}" > "${LOG}" 2>&1 &
        SERVER_PID=$!
        ;;
esac
trap 'kill ${SERVER_PID} 2>/dev/null || true; echo "logs at ${OUTDIR}"' EXIT

# Wait for bind. Two modes:
#
#   USE_ANNOUNCE=1 (opt-in)  — read the actually-bound port back from
#     the server's stdout LOG, parsing the
#     `listening on <host>:<port>` line emitted by the
#     `TAIDA_NET_ANNOUNCE_PORT=1` opt-in surface (C27B-014). This is
#     what unblocks the port=0 / OS-assigned-port flow the soak
#     runbook (`.dev/C26_SOAK_RUNBOOK.md § 2.1`) needs. PORT is
#     overwritten with the announced value.
#
#   USE_ANNOUNCE unset / =0  — keep the legacy fixed-port TCP probe.
#     This is the default for the per-backend fixed ports
#     (interp=18081 / js=18082 / native=18083) so existing CI smoke
#     and parallel multi-backend runs keep working unchanged.
#
# Either path fails fast if the server process exits before we get a
# usable signal (parse error / panic / bind error).
if [ "${USE_ANNOUNCE:-0}" = "1" ]; then
    ANNOUNCED=""
    for _ in $(seq 1 150); do
        if [ -s "${LOG}" ]; then
            ANNOUNCED=$(grep -m1 -oE 'listening on [^[:space:]]+:[0-9]+' "${LOG}" 2>/dev/null | awk '{print $3}' || true)
            if [ -n "${ANNOUNCED}" ]; then
                break
            fi
        fi
        if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
            echo "server exited before announce" >&2
            cat "${LOG}" >&2
            exit 2
        fi
        sleep 0.2
    done
    if [ -z "${ANNOUNCED}" ]; then
        echo "server did not announce a listening port within 30s (USE_ANNOUNCE=1)" >&2
        echo "hint: ensure TAIDA_NET_ANNOUNCE_PORT=1 is set for the spawned server" >&2
        cat "${LOG}" >&2
        exit 2
    fi
    PORT="${ANNOUNCED##*:}"
    echo "  server listening on ${ANNOUNCED} (pid=${SERVER_PID}, USE_ANNOUNCE=1)"
else
    READY=0
    for _ in $(seq 1 60); do
        if (exec 3<>/dev/tcp/127.0.0.1/${PORT}) 2>/dev/null; then
            exec 3>&- 3<&-
            READY=1
            break
        fi
        # Fail fast if the server already died (parse error etc.).
        if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
            echo "server exited before bind" >&2
            cat "${LOG}" >&2
            exit 2
        fi
        sleep 0.5
    done
    if [ "${READY}" -ne 1 ]; then
        echo "server did not bind 127.0.0.1:${PORT} within 30s" >&2
        cat "${LOG}" >&2
        exit 2
    fi
    echo "  server listening on 127.0.0.1:${PORT} (pid=${SERVER_PID})"
    # If the server emitted a `listening on …` line to its LOG (the
    # opt-in announcement enabled by TAIDA_NET_ANNOUNCE_PORT=1, which
    # this script sets inline on every backend launch), surface it on
    # the proxy's own stdout. CI (`.github/workflows/soak-smoke.yml`)
    # asserts on this line as a plumbing canary for C27B-014; local
    # operators get a confirmation line they can paste into the runbook.
    # Stays a no-op if the env var was filtered out by some wrapper.
    if [ -s "${LOG}" ]; then
        ANNOUNCE_LINE=$(grep -m1 -E 'listening on [^[:space:]]+:[0-9]+' "${LOG}" 2>/dev/null || true)
        if [ -n "${ANNOUNCE_LINE}" ]; then
            echo "  server-announce: ${ANNOUNCE_LINE}"
        fi
    fi
fi

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
