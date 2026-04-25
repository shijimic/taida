#!/usr/bin/env bash
# D28B-013 acceptance #2: Peak RSS regression hard-fail gate.
#
# Measures the peak RSS of a `taida` invocation against each fixture
# in `examples/quality/d28_perf_smoke/` using `/usr/bin/time -v`.
# Emits bencher-format lines that the existing
# `scripts/bench/compare_baseline.py` can ingest, so the same
# tolerance + min-samples state machine guards both throughput and
# RSS regressions.
#
# Usage:
#   scripts/perf/measure_peak_rss.sh <path-to-taida-binary> [fixture]
#
#   With no fixture argument: iterates over every
#   examples/quality/d28_perf_smoke/*.td fixture, emits one
#   bencher-format line per fixture to stdout, exits 0.
#
#   With a fixture argument (single .td path): measures that
#   fixture only.
#
# Exit codes:
#   0  — measurements collected (or all `--check-against-baseline`
#        comparisons passed)
#   1  — at least one fixture exited non-zero, or
#        `--check-against-baseline` reported a regression
#   2  — usage / environment error
#
# Bencher output line format (matches what
# `scripts/bench/compare_baseline.py` parses; ns is reused as the
# unit slot but the value is RSS in **kibibytes** to keep integers
# bounded — gate compares relative %, units cancel):
#   test rss_<fixture_name> ... bench: <peak_rss_kib> ns/iter (+/- 0)
#
# This is intentional: we reuse the existing bencher-format gate
# infra rather than writing a parallel one. The "ns" slot is just a
# numeric column that cmp tools treat as units-agnostic. Documented
# in scripts/perf/README.md.

set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <path-to-taida-binary> [fixture-or-flag]" >&2
  echo "       $0 <path-to-taida-binary> --check-against-baseline <baseline.json>" >&2
  exit 2
fi

BIN="${1}"
shift || true

if [[ ! -x "${BIN}" ]]; then
  echo "error: taida binary not found or not executable: ${BIN}" >&2
  exit 2
fi

if ! command -v /usr/bin/time >/dev/null 2>&1; then
  echo "error: /usr/bin/time not found. Install via 'sudo apt-get install -y time'." >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FIXTURE_DIR="${REPO_ROOT}/examples/quality/d28_perf_smoke"
LOG_DIR="${PEAK_RSS_LOG_DIR:-${REPO_ROOT}/target/perf-rss}"
mkdir -p "${LOG_DIR}"

CHECK_BASELINE=""
SINGLE_FIXTURE=""
if [[ $# -ge 1 ]]; then
  case "${1}" in
    --check-against-baseline)
      shift
      if [[ $# -lt 1 ]]; then
        echo "error: --check-against-baseline requires a baseline JSON path" >&2
        exit 2
      fi
      CHECK_BASELINE="${1}"
      shift || true
      ;;
    *.td)
      SINGLE_FIXTURE="${1}"
      ;;
    *)
      echo "error: unrecognised argument '${1}'. expected --check-against-baseline <path> or a .td fixture path" >&2
      exit 2
      ;;
  esac
fi

if [[ ! -d "${FIXTURE_DIR}" ]]; then
  echo "error: fixture directory missing: ${FIXTURE_DIR}" >&2
  echo "       create examples/quality/d28_perf_smoke/ and add at least one .td fixture." >&2
  exit 2
fi

if [[ -n "${SINGLE_FIXTURE}" ]]; then
  FIXTURES=("${SINGLE_FIXTURE}")
else
  shopt -s nullglob
  FIXTURES=("${FIXTURE_DIR}"/*.td)
fi

if [[ ${#FIXTURES[@]} -eq 0 ]]; then
  echo "error: no fixtures to measure" >&2
  exit 2
fi

OUTPUT_FILE="${LOG_DIR}/peak_rss_results.txt"
: > "${OUTPUT_FILE}"

failed=0

for fixture in "${FIXTURES[@]}"; do
  name="$(basename "${fixture}" .td)"
  log="${LOG_DIR}/${name}.time.log"

  # `/usr/bin/time -v` writes resource usage to stderr.
  set +e
  /usr/bin/time -v -o "${log}" "${BIN}" "${fixture}" >/dev/null 2>&1
  rc=$?
  set -e

  if [[ ${rc} -ne 0 ]]; then
    echo "FAIL :: ${name} (rc=${rc})" >&2
    failed=$((failed + 1))
    continue
  fi

  # Maximum resident set size in kibibytes.
  rss_kib="$(grep -E 'Maximum resident set size' "${log}" | sed -E 's/.*: ([0-9]+)/\1/' || true)"
  rss_kib="${rss_kib:-0}"

  if [[ "${rss_kib}" -le 0 ]]; then
    echo "FAIL :: ${name} (could not parse peak RSS from ${log})" >&2
    failed=$((failed + 1))
    continue
  fi

  echo "OK   :: ${name} peak_rss=${rss_kib}KiB"
  # Emit bencher-format line so compare_baseline.py can ingest.
  echo "test rss_${name} ... bench: ${rss_kib} ns/iter (+/- 0)" >> "${OUTPUT_FILE}"
done

if [[ ${failed} -gt 0 ]]; then
  echo ""
  echo "=== peak RSS measurement FAILED: ${failed}/${#FIXTURES[@]} fixture(s) crashed or unparseable ==="
  exit 1
fi

cat "${OUTPUT_FILE}"

if [[ -n "${CHECK_BASELINE}" ]]; then
  if [[ ! -f "${CHECK_BASELINE}" ]]; then
    echo "error: baseline JSON not found: ${CHECK_BASELINE}" >&2
    exit 2
  fi
  python3 "${REPO_ROOT}/scripts/bench/compare_baseline.py" \
      --bencher-out "${OUTPUT_FILE}" \
      --baseline "${CHECK_BASELINE}" \
      --tolerance-pct 10.0 \
      --min-samples 30
fi

exit 0
