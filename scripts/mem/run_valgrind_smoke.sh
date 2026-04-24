#!/usr/bin/env bash
# C26B-010: Memory leak detection gate for interpreter smoke fixtures.
#
# Owned by .github/workflows/memory.yml. Runs valgrind with a strict
# `--error-exitcode=1` + `--errors-for-leak-kinds=definite` policy against
# the smoke fixtures in examples/quality/c26_mem_smoke/ and fails the
# job on any `definitely lost` byte. Indirectly-lost / possibly-lost
# bytes are surfaced in the summary but do not hard-fail (those are
# commonly dominated by one-shot global allocations like the tokio
# runtime thread-pool, which are not regression signals).
#
# Usage:
#   scripts/mem/run_valgrind_smoke.sh <path-to-taida-binary>
#
# Exit code:
#   0  — all fixtures produced zero `definitely lost` bytes.
#   1  — valgrind reported at least one definitely-lost allocation, OR
#        a fixture crashed / exited non-zero.
#   2  — usage / environment error (no valgrind, binary missing, etc).

set -euo pipefail

BIN="${1:-}"
if [[ -z "${BIN}" ]]; then
  echo "usage: $0 <path-to-taida-binary>" >&2
  exit 2
fi
if [[ ! -x "${BIN}" ]]; then
  echo "error: taida binary not found or not executable: ${BIN}" >&2
  exit 2
fi
if ! command -v valgrind >/dev/null 2>&1; then
  echo "error: valgrind not found on PATH. install via 'sudo apt-get install -y valgrind'." >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FIXTURE_DIR="${REPO_ROOT}/examples/quality/c26_mem_smoke"
LOG_DIR="${VALGRIND_LOG_DIR:-${REPO_ROOT}/target/mem-smoke}"
mkdir -p "${LOG_DIR}"

if [[ ! -d "${FIXTURE_DIR}" ]]; then
  echo "error: fixture directory missing: ${FIXTURE_DIR}" >&2
  exit 2
fi

shopt -s nullglob
FIXTURES=("${FIXTURE_DIR}"/*.td)
if [[ ${#FIXTURES[@]} -eq 0 ]]; then
  echo "error: no .td fixtures in ${FIXTURE_DIR}" >&2
  exit 2
fi

total_fixtures=${#FIXTURES[@]}
failed=0
failed_names=()

summary_rows=()

for fixture in "${FIXTURES[@]}"; do
  name="$(basename "${fixture}" .td)"
  log="${LOG_DIR}/${name}.valgrind.log"
  stdout_log="${LOG_DIR}/${name}.stdout.log"
  echo "--- valgrind :: ${name} ---"

  # `--error-exitcode=1` only trips when `--errors-for-leak-kinds` matches,
  # so `definite` keeps the gate narrow. `--child-silent-after-exec=yes`
  # prevents any wrapped subprocess (e.g. a JS sidecar spawned by the
  # interpreter) from polluting the log with redundant leak reports.
  set +e
  valgrind \
    --tool=memcheck \
    --leak-check=full \
    --show-leak-kinds=definite \
    --errors-for-leak-kinds=definite \
    --error-exitcode=1 \
    --child-silent-after-exec=yes \
    --log-file="${log}" \
    --quiet \
    "${BIN}" "${fixture}" > "${stdout_log}" 2>&1
  rc=$?
  set -e

  # Extract the definitely-lost byte count for the summary, even on success.
  definite_bytes="$(grep -E 'definitely lost:' "${log}" | head -n1 | sed -E 's/.*definitely lost:\s*([0-9,]+)\s*bytes.*/\1/' | tr -d ',' || true)"
  definite_bytes="${definite_bytes:-0}"
  indirect_bytes="$(grep -E 'indirectly lost:' "${log}" | head -n1 | sed -E 's/.*indirectly lost:\s*([0-9,]+)\s*bytes.*/\1/' | tr -d ',' || true)"
  indirect_bytes="${indirect_bytes:-0}"
  possible_bytes="$(grep -E 'possibly lost:' "${log}" | head -n1 | sed -E 's/.*possibly lost:\s*([0-9,]+)\s*bytes.*/\1/' | tr -d ',' || true)"
  possible_bytes="${possible_bytes:-0}"

  if [[ ${rc} -ne 0 ]]; then
    failed=$((failed + 1))
    failed_names+=("${name}")
    echo "FAIL :: ${name} (rc=${rc}, definite=${definite_bytes}B, indirect=${indirect_bytes}B, possibly=${possible_bytes}B)"
    echo "       see ${log}"
    # Dump captured stdout/stderr and the valgrind log so CI runs are
    # debuggable when the smoke crashes before valgrind can write its
    # leak summary (e.g. unsupported syscall on a newer runner kernel).
    echo "--- ${name} stdout/stderr (captured) ---"
    if [[ -s "${stdout_log}" ]]; then
      sed 's/^/  /' "${stdout_log}"
    else
      echo "  (empty)"
    fi
    echo "--- ${name} valgrind log ---"
    if [[ -s "${log}" ]]; then
      sed 's/^/  /' "${log}"
    else
      echo "  (missing or empty — valgrind exited before writing)"
    fi
    echo "--- end ${name} ---"
  else
    echo "OK   :: ${name} (definite=0B, indirect=${indirect_bytes}B, possibly=${possible_bytes}B)"
  fi
  summary_rows+=("${name}|${rc}|${definite_bytes}|${indirect_bytes}|${possible_bytes}")
done

# GitHub Actions step summary (written only if $GITHUB_STEP_SUMMARY is set).
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  {
    echo "## valgrind smoke (C26B-010)"
    echo
    echo "Policy: fail on any \`definitely lost\` byte. indirect / possibly lost are"
    echo "surfaced for visibility only."
    echo
    echo "| fixture | exit | definite | indirect | possibly |"
    echo "|---|---:|---:|---:|---:|"
    for row in "${summary_rows[@]}"; do
      IFS='|' read -r n rc d i p <<<"${row}"
      echo "| \`${n}\` | ${rc} | ${d} | ${i} | ${p} |"
    done
    echo
    echo "Ran ${total_fixtures} fixtures. Failed: ${failed}."
  } >> "${GITHUB_STEP_SUMMARY}"
fi

if [[ ${failed} -gt 0 ]]; then
  echo ""
  echo "=== valgrind smoke FAILED: ${failed}/${total_fixtures} fixture(s) leaked or crashed ==="
  for n in "${failed_names[@]}"; do
    echo "  - ${n}"
  done
  exit 1
fi

echo ""
echo "=== valgrind smoke PASSED: ${total_fixtures}/${total_fixtures} fixtures clean ==="
exit 0
