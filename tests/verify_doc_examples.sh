#!/usr/bin/env bash
# RC-1f: Verify documentation code examples
#
# Runs all doc-derived quality test files (d1a-d2g, rc1a-*) and compares
# their output against .expected files.
#
# These test files are manually extracted from docs/guide/ and docs/reference/
# to ensure documentation code examples actually work as described.
#
# Usage:
#   ./tests/verify_doc_examples.sh
#   TAIDA_BIN="$PWD/target/debug/taida" ./tests/verify_doc_examples.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd -P)"
QUALITY_DIR="$PROJECT_DIR/examples/quality"
source "$SCRIPT_DIR/scripts/lib_taida_bin.sh"
TAIDA="$(resolve_taida_bin)" || exit 1

PASS=0
FAIL=0
SKIP=0
ERRORS=""

# Determine timeout command (macOS ships without `timeout`; use `gtimeout` from coreutils)
if command -v timeout >/dev/null 2>&1; then
  TIMEOUT_CMD="timeout"
elif command -v gtimeout >/dev/null 2>&1; then
  TIMEOUT_CMD="gtimeout"
else
  # No timeout available — run without timeout wrapper
  TIMEOUT_CMD=""
fi

# Colors (disable if not a terminal)
if [ -t 1 ]; then
  GREEN='\033[0;32m'
  RED='\033[0;31m'
  YELLOW='\033[0;33m'
  CYAN='\033[0;36m'
  NC='\033[0m'
else
  GREEN=''
  RED=''
  YELLOW=''
  CYAN=''
  NC=''
fi

pass() {
  PASS=$((PASS + 1))
  printf "${GREEN}PASS${NC} %s\n" "$1"
}

fail() {
  FAIL=$((FAIL + 1))
  printf "${RED}FAIL${NC} %s\n" "$1"
  if [ -n "${2:-}" ]; then
    ERRORS="${ERRORS}--- $1 ---\n$2\n\n"
  fi
}

skip() {
  SKIP=$((SKIP + 1))
  printf "${YELLOW}SKIP${NC} %s\n" "$1"
}

# =========================================================================
# Section 1: docs/guide/ examples (d1a-d1m)
# =========================================================================
printf "\n${CYAN}=== Section 1: docs/guide/ Code Examples ===${NC}\n"

# Map test files to their source docs
declare -A GUIDE_MAP
GUIDE_MAP[d1a_overview]="00_overview.md"
GUIDE_MAP[d1b_types]="01_types.md"
GUIDE_MAP[d1c_strict_typing]="02_strict_typing.md"
GUIDE_MAP[d1d_json]="03_json.md"
GUIDE_MAP[d1e_buchi_pack]="04_buchi_pack.md"
GUIDE_MAP[d1f_molding]="05_molding.md"
GUIDE_MAP[d1g_lists]="06_lists.md"
GUIDE_MAP[d1h_control_flow]="07_control_flow.md"
GUIDE_MAP[d1i_error_handling]="08_error_handling.md"
GUIDE_MAP[d1j_functions]="09_functions.md"
GUIDE_MAP[d1k_modules]="10_modules.md"
GUIDE_MAP[d1l_async]="11_async.md"
GUIDE_MAP[d1m_introspection]="12_introspection.md"

for test_name in d1a_overview d1b_types d1c_strict_typing d1d_json d1e_buchi_pack d1f_molding d1g_lists d1h_control_flow d1i_error_handling d1j_functions d1k_modules d1l_async d1m_introspection; do
  td_file="$QUALITY_DIR/${test_name}.td"
  expected_file="$QUALITY_DIR/${test_name}.expected"
  doc_source="${GUIDE_MAP[$test_name]:-unknown}"

  if [ ! -f "$td_file" ]; then
    skip "$test_name ($doc_source) -- test file not found"
    continue
  fi
  if [ ! -f "$expected_file" ]; then
    skip "$test_name ($doc_source) -- .expected file not found"
    continue
  fi

  # Run and capture output (stderr is captured separately for diagnostics)
  stderr_file=$(mktemp)
  actual=$(${TIMEOUT_CMD:+$TIMEOUT_CMD 10} "$TAIDA" "$td_file" 2>"$stderr_file") || {
    stderr_content=$(cat "$stderr_file")
    rm -f "$stderr_file"
    if echo "$stderr_content" | grep -qi "parse error\|syntax error\|compile error\|lowering error"; then
      fail "$test_name ($doc_source) -- compile error" "$stderr_content"
    else
      fail "$test_name ($doc_source) -- runtime error" "$stderr_content"
    fi
    continue
  }
  rm -f "$stderr_file"

  expected=$(cat "$expected_file")

  if [ "$actual" = "$expected" ]; then
    pass "$test_name ($doc_source)"
  else
    diff_output=$(diff <(echo "$actual") <(echo "$expected") || true)
    fail "$test_name ($doc_source) -- output mismatch" "$diff_output"
  fi
done

# =========================================================================
# Section 2: docs/reference/ examples (d2a-d2g)
# =========================================================================
printf "\n${CYAN}=== Section 2: docs/reference/ Code Examples ===${NC}\n"

declare -A REF_MAP
REF_MAP[d2a_operators]="operators.md"
REF_MAP[d2b_mold_types]="mold_types.md"
REF_MAP[d2c_standard_methods]="standard_methods.md"
REF_MAP[d2d_standard_library]="standard_library.md"
REF_MAP[d2e_tail_recursion]="tail_recursion.md"
REF_MAP[d2f_scope_rules]="scope_rules.md"
REF_MAP[d2g_naming_conventions]="naming_conventions.md"

for test_name in d2a_operators d2b_mold_types d2c_standard_methods d2d_standard_library d2e_tail_recursion d2f_scope_rules d2g_naming_conventions; do
  td_file="$QUALITY_DIR/${test_name}.td"
  expected_file="$QUALITY_DIR/${test_name}.expected"
  doc_source="${REF_MAP[$test_name]:-unknown}"

  if [ ! -f "$td_file" ]; then
    skip "$test_name ($doc_source) -- test file not found"
    continue
  fi
  if [ ! -f "$expected_file" ]; then
    skip "$test_name ($doc_source) -- .expected file not found"
    continue
  fi

  stderr_file=$(mktemp)
  actual=$(${TIMEOUT_CMD:+$TIMEOUT_CMD 10} "$TAIDA" "$td_file" 2>"$stderr_file") || {
    stderr_content=$(cat "$stderr_file")
    rm -f "$stderr_file"
    if echo "$stderr_content" | grep -qi "parse error\|syntax error\|compile error\|lowering error"; then
      fail "$test_name ($doc_source) -- compile error" "$stderr_content"
    else
      fail "$test_name ($doc_source) -- runtime error" "$stderr_content"
    fi
    continue
  }
  rm -f "$stderr_file"

  expected=$(cat "$expected_file")

  if [ "$actual" = "$expected" ]; then
    pass "$test_name ($doc_source)"
  else
    diff_output=$(diff <(echo "$actual") <(echo "$expected") || true)
    fail "$test_name ($doc_source) -- output mismatch" "$diff_output"
  fi
done

# =========================================================================
# Section 3: RC-1 spec audit tests (rc1a-*)
# =========================================================================
printf "\n${CYAN}=== Section 3: RC-1 Specification Audit Tests ===${NC}\n"

for td_file in "$QUALITY_DIR"/rc1a_*.td; do
  base=$(basename "$td_file" .td)
  expected_file="$QUALITY_DIR/${base}.expected"

  if [ ! -f "$expected_file" ]; then
    skip "$base -- .expected file not found"
    continue
  fi

  stderr_file=$(mktemp)
  actual=$(${TIMEOUT_CMD:+$TIMEOUT_CMD 10} "$TAIDA" "$td_file" 2>"$stderr_file") || {
    stderr_content=$(cat "$stderr_file")
    rm -f "$stderr_file"
    if echo "$stderr_content" | grep -qi "parse error\|syntax error\|compile error\|lowering error"; then
      fail "$base -- compile error" "$stderr_content"
    else
      fail "$base -- runtime error" "$stderr_content"
    fi
    continue
  }
  rm -f "$stderr_file"

  expected=$(cat "$expected_file")

  if [ "$actual" = "$expected" ]; then
    pass "$base"
  else
    diff_output=$(diff <(echo "$actual") <(echo "$expected") || true)
    fail "$base -- output mismatch" "$diff_output"
  fi
done

# =========================================================================
# Section 4: Coverage Report
# =========================================================================
printf "\n${CYAN}=== Section 4: Documentation Coverage ===${NC}\n"

guide_total=13
guide_tested=$(ls "$QUALITY_DIR"/d1*.td 2>/dev/null | grep -v _lib | wc -l)
printf "  docs/guide/  : %d/%d chapters have executable tests\n" "$guide_tested" "$guide_total"

ref_total=7
ref_tested=$(ls "$QUALITY_DIR"/d2*.td 2>/dev/null | wc -l)
printf "  docs/reference/: %d/%d documents have executable tests\n" "$ref_tested" "$ref_total"

spec_tests=$(ls "$QUALITY_DIR"/rc1a_*.td 2>/dev/null | wc -l)
printf "  RC-1 audit   : %d spec audit test files\n" "$spec_tests"

# =========================================================================
# Summary
# =========================================================================
printf "\n=========================================\n"
printf "Results: %d passed, %d failed, %d skipped\n" "$PASS" "$FAIL" "$SKIP"
printf "=========================================\n"

if [ -n "$ERRORS" ]; then
  printf "\n=== Failure Details ===\n"
  printf '%b' "$ERRORS"
fi

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi

exit 0
