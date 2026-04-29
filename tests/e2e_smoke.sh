#!/usr/bin/env bash
# E2E Smoke Tests for Taida Lang
# Runs selected examples and compares output against expected files.
# Also tests CLI commands (verify, graph, type-check).
#
# Usage:
#   ./tests/e2e_smoke.sh              # uses cargo run
#   TAIDA_BIN=./target/release/taida ./tests/e2e_smoke.sh  # uses prebuilt binary

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
EXPECTED_DIR="$SCRIPT_DIR/expected"

# Determine the taida binary
if [ -n "${TAIDA_BIN:-}" ]; then
  TAIDA="$TAIDA_BIN"
elif [ -f "$PROJECT_DIR/target/release/taida" ]; then
  TAIDA="$PROJECT_DIR/target/release/taida"
elif [ -f "$PROJECT_DIR/target/debug/taida" ]; then
  TAIDA="$PROJECT_DIR/target/debug/taida"
else
  # Fall back to cargo run
  TAIDA="cargo run --quiet --"
fi

PASS=0
FAIL=0
SKIP=0
ERRORS=""

# Colors (disable if not a terminal)
if [ -t 1 ]; then
  GREEN='\033[0;32m'
  RED='\033[0;31m'
  YELLOW='\033[0;33m'
  NC='\033[0m'
else
  GREEN=''
  RED=''
  YELLOW=''
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
# Section 1: Example output comparison
# =========================================================================
echo "=== Section 1: Example Output Tests ==="

for expected_file in "$EXPECTED_DIR"/*.txt; do
  basename=$(basename "$expected_file" .txt)
  td_file="$PROJECT_DIR/examples/${basename}.td"

  if [ ! -f "$td_file" ]; then
    skip "$basename (source file not found)"
    continue
  fi

  # Run the example and capture output
  actual=$($TAIDA "$td_file" 2>/dev/null) || {
    fail "$basename (runtime error)"
    continue
  }

  expected=$(cat "$expected_file")

  # Compare (ignore trailing whitespace differences)
  if [ "$(echo "$actual" | sed 's/[[:space:]]*$//')" = "$(echo "$expected" | sed 's/[[:space:]]*$//')" ]; then
    pass "$basename"
  else
    diff_output=$(diff <(echo "$actual") <(echo "$expected") || true)
    fail "$basename (output mismatch)" "$diff_output"
  fi
done

echo ""

# =========================================================================
# Section 2: Type-check all examples
# =========================================================================
echo "=== Section 2: Type-Check Examples ==="

# Use `taida way check` (parse + typecheck only, no execution) so that server /
# interactive / network-bound examples like `httpServe(...)` /
# `wsReceive(...)` / raw-mode terminal fixtures cannot hang the smoke job
# waiting for I/O. The previous form `$TAIDA "$td_file"` actually executed
# the file — that conflated typecheck with run and required a per-fixture
# skip list (kept symmetric with WASI_SKIP_STEMS / FULL_SKIP_STEMS) that
# rotted every time a new server / interactive example landed (most
# recently the D28B-017/018 set). `taida way check` finishes in O(ms) for
# every example regardless of what runtime I/O it would trigger.
#
# Known-broken legacy fixtures (kept matching the same exclusion list used
# by tests/d28b_008_lint_invariants.rs so that one decision tracks them):
CHECK_SKIP_STEMS="addon_terminal"

typecheck_pass=0
typecheck_fail=0
for td_file in "$PROJECT_DIR"/examples/*.td; do
  basename=$(basename "$td_file")
  stem="${basename%.td}"
  skip_this=0
  for s in $CHECK_SKIP_STEMS; do
    if [ "$stem" = "$s" ]; then skip_this=1; break; fi
  done
  if [ "$skip_this" = "1" ]; then
    skip "typecheck: $basename (known-broken legacy fixture — listed in CHECK_SKIP_STEMS)"
    continue
  fi

  if $TAIDA way check "$td_file" >/dev/null 2>&1; then
    typecheck_pass=$((typecheck_pass + 1))
  else
    fail "typecheck: $basename (taida way check failed)"
    typecheck_fail=$((typecheck_fail + 1))
  fi
done

if [ "$typecheck_fail" -eq 0 ]; then
  pass "All examples pass type-check ($typecheck_pass files)"
fi

echo ""

# =========================================================================
# Section 3: --no-check flag
# =========================================================================
echo "=== Section 3: --no-check Flag ==="

actual=$($TAIDA --no-check "$PROJECT_DIR/examples/01_hello.td" 2>/dev/null)
if [ "$actual" = "Hello, Taida Lang!" ]; then
  pass "--no-check flag works"
else
  fail "--no-check flag (unexpected output)"
fi

echo ""

# =========================================================================
# Section 4: verify command
# =========================================================================
echo "=== Section 4: Verify Command ==="

verify_output=$($TAIDA way verify "$PROJECT_DIR/examples/01_hello.td" 2>/dev/null)
if echo "$verify_output" | grep -q "passed"; then
  pass "verify command runs successfully"
else
  fail "verify command" "$verify_output"
fi

# Test specific check
verify_check=$($TAIDA way verify --check error-coverage "$PROJECT_DIR/examples/08_error_handling.td" 2>/dev/null)
if echo "$verify_check" | grep -q "PASS"; then
  pass "verify --check error-coverage"
else
  fail "verify --check error-coverage" "$verify_check"
fi

echo ""

# =========================================================================
# Section 5: graph command
# =========================================================================
echo "=== Section 5: Graph Command ==="

# Test graph AI JSON output (unified format, no --type/--format flags)
graph_output=$($TAIDA graph "$PROJECT_DIR/examples/04_functions.td" 2>/dev/null)
if [ $? -eq 0 ] && echo "$graph_output" | grep -q '"taida_version"'; then
  pass "graph AI JSON output"
else
  fail "graph AI JSON output" "$graph_output"
fi

# Test graph JSON contains expected sections
if echo "$graph_output" | grep -q '"functions"' && echo "$graph_output" | grep -q '"types"'; then
  pass "graph JSON has functions and types"
else
  fail "graph JSON has functions and types" "$graph_output"
fi

# Test graph with -o flag
graph_tmp=$(mktemp)
$TAIDA graph -o "$graph_tmp" "$PROJECT_DIR/examples/04_functions.td" 2>/dev/null
if [ $? -eq 0 ] && grep -q '"taida_version"' "$graph_tmp"; then
  pass "graph -o output file"
else
  fail "graph -o output file"
fi
rm -f "$graph_tmp"

# Test way verify --format sarif
sarif_output=$($TAIDA way verify --format sarif "$PROJECT_DIR/examples/04_functions.td" 2>/dev/null)
if echo "$sarif_output" | grep -q '"version": "2.1.0"'; then
  pass "way verify --format sarif"
else
  fail "way verify --format sarif" "$sarif_output"
fi

# Test graph summary command
summary_output=$($TAIDA graph summary "$PROJECT_DIR/examples/04_functions.td" 2>/dev/null)
if echo "$summary_output" | grep -q '"version": "1.0"' && ! echo "$summary_output" | grep -q '"verification"'; then
  pass "graph summary command"
else
  fail "graph summary command" "$summary_output"
fi

echo ""

# =========================================================================
# Section 6: Error handling
# =========================================================================
echo "=== Section 6: Error Handling ==="

# Div mold with zero returns Lax(hasValue=false), unmold gives default 0
div_zero_src='
r <= Div[10, 0]()
stdout(r.hasValue.toString())
r ]=> val
stdout(val.toString())
'
div_zero_output=$(echo "$div_zero_src" | $TAIDA /dev/stdin 2>/dev/null)
if echo "$div_zero_output" | grep -q "false"; then
  pass "Div[10, 0]() returns Lax(hasValue=false)"
else
  fail "Div[10, 0]() returns Lax(hasValue=false)" "got: $div_zero_output"
fi

# Out of bounds get() returns Lax with hasValue=false
oob_src='
items <= @[1, 2, 3]
lax <= items.get(100)
stdout(lax.hasValue.toString())
lax ]=> val
stdout(val.toString())
'
oob_output=$(echo "$oob_src" | $TAIDA /dev/stdin 2>/dev/null)
if echo "$oob_output" | head -1 | grep -q "false"; then
  pass "OOB get() returns Lax(hasValue=false)"
else
  fail "OOB get() returns Lax(hasValue=false)" "got: $oob_output"
fi

echo ""

# =========================================================================
# Section 7: JS Build (target=js) + Node Execution
# =========================================================================
echo "=== Section 7: JS Build Tests ==="

# Check if node is available
if command -v node >/dev/null 2>&1; then
  TMPDIR_JS=$(mktemp -d)
  js_pass=0
  js_fail=0
  js_skip=0

  for i in $(seq -w 1 20); do
    td_file=""
    for candidate in "$PROJECT_DIR"/examples/"${i}"*.td; do
      if [ -f "$candidate" ]; then
        td_file="$candidate"
        break
      fi
    done
    if [ -z "$td_file" ]; then
      continue
    fi
    basename=$(basename "$td_file" .td)

    # Build JS (single-file mode)
    if ! $TAIDA build js "$td_file" -o "$TMPDIR_JS/${basename}.js" >/dev/null 2>&1; then
      fail "js-build: $basename (build failed)"
      js_fail=$((js_fail + 1))
      continue
    fi

    # For module example, also build the module
    if [ "$basename" = "09_modules" ]; then
      $TAIDA build js "$PROJECT_DIR/examples/module_utils.td" -o "$TMPDIR_JS/module_utils.mjs" >/dev/null 2>&1 || true
    fi

    # Execute with node
    actual=$(node "$TMPDIR_JS/${basename}.js" 2>&1) || {
      fail "js-build: $basename (node execution failed)"
      js_fail=$((js_fail + 1))
      continue
    }

    # For examples with dynamic output (time, I/O), just verify execution succeeded

    # Compare with interpreter output
    expected=$($TAIDA "$td_file" 2>/dev/null) || {
      skip "js-build: $basename (interpreter failed, cannot compare)"
      js_skip=$((js_skip + 1))
      continue
    }

    # Normalize float formatting: JS omits .0 for integer-valued floats.
    # RCB-47: Use a normalize function that handles all "= <value>" positions
    # in a line, not just the line-final one.
    actual_norm=$(echo "$actual" | sed 's/[[:space:]]*$//')
    expected_norm=$(echo "$expected" | sed 's/[[:space:]]*$//')

    if [ "$actual_norm" = "$expected_norm" ]; then
      pass "js-build: $basename"
      js_pass=$((js_pass + 1))
    else
      # Allow float formatting differences (e.g. "4" vs "4.0", "4.10" vs "4.1").
      # Step 1: Append .0 to bare integers after "= " or "= -" (handles mid-line and negatives).
      # Step 2: Collapse ".00…0" (all-zero decimals) to ".0" (e.g. "4.00" -> "4.0").
      # Step 3: Strip trailing zeros after a non-zero decimal digit (e.g. "4.10" -> "4.1").
      # Step 4: Strip lone trailing dot (e.g. "4." -> "4.0").
      actual_relaxed=$(echo "$actual_norm" | \
        sed 's/= \(-\{0,1\}[0-9][0-9]*\)\([^.0-9]\)/= \1.0\2/g; s/= \(-\{0,1\}[0-9][0-9]*\)$/= \1.0/' | \
        sed 's/\.\(0\{2,\}\)$/.0/g; s/\.\(0\{2,\}\)\([^0-9]\)/.0\2/g' | \
        sed 's/\(\.[0-9]*[1-9]\)0\{1,\}/\1/g' | \
        sed 's/= \(-\{0,1\}[0-9][0-9]*\)\.$/= \1.0/g')
      expected_relaxed=$(echo "$expected_norm" | \
        sed 's/= \(-\{0,1\}[0-9][0-9]*\)\([^.0-9]\)/= \1.0\2/g; s/= \(-\{0,1\}[0-9][0-9]*\)$/= \1.0/' | \
        sed 's/\.\(0\{2,\}\)$/.0/g; s/\.\(0\{2,\}\)\([^0-9]\)/.0\2/g' | \
        sed 's/\(\.[0-9]*[1-9]\)0\{1,\}/\1/g' | \
        sed 's/= \(-\{0,1\}[0-9][0-9]*\)\.$/= \1.0/g')
      if [ "$actual_relaxed" = "$expected_relaxed" ]; then
        pass "js-build: $basename (float format tolerance)"
        js_pass=$((js_pass + 1))
      else
        diff_output=$(diff <(echo "$actual") <(echo "$expected") || true)
        fail "js-build: $basename (output mismatch)" "$diff_output"
        js_fail=$((js_fail + 1))
      fi
    fi
  done

  # ── Additional JS build tests: complex examples ──
  for extra_td in todo_app api_client; do
    td_file="$PROJECT_DIR/examples/${extra_td}.td"
    if [ ! -f "$td_file" ]; then
      js_skip=$((js_skip + 1))
      continue
    fi

    # Build JS (single-file mode)
    if ! $TAIDA build js "$td_file" -o "$TMPDIR_JS/${extra_td}.js" >/dev/null 2>&1; then
      fail "js-build: $extra_td (build failed)"
      js_fail=$((js_fail + 1))
      continue
    fi

    # Execute with node
    actual=$(node "$TMPDIR_JS/${extra_td}.js" 2>&1) || {
      fail "js-build: $extra_td (node execution failed)"
      js_fail=$((js_fail + 1))
      continue
    }

    # Compare with interpreter output
    expected=$($TAIDA "$td_file" 2>/dev/null) || {
      skip "js-build: $extra_td (interpreter failed, cannot compare)"
      js_skip=$((js_skip + 1))
      continue
    }

    actual_norm=$(echo "$actual" | sed 's/[[:space:]]*$//')
    expected_norm=$(echo "$expected" | sed 's/[[:space:]]*$//')

    if [ "$actual_norm" = "$expected_norm" ]; then
      pass "js-build: $extra_td"
      js_pass=$((js_pass + 1))
    else
      diff_output=$(diff <(echo "$actual") <(echo "$expected") || true)
      fail "js-build: $extra_td (output mismatch)" "$diff_output"
      js_fail=$((js_fail + 1))
    fi
  done

  rm -rf "$TMPDIR_JS"
  echo "  JS build: $js_pass passed, $js_fail failed, $js_skip skipped"
else
  skip "JS build tests (node not available)"
fi

echo ""

# =========================================================================
# Section 8: Lax Fallback API (v0.5.0)
# =========================================================================
echo "=== Section 8: Lax Fallback API ==="

# list.get() returns Lax
get_src='
items <= @[10, 20, 30]
lax <= items.get(1)
stdout(lax.hasValue.toString())
lax2 <= items.get(100)
stdout(lax2.hasValue.toString())
'
get_output=$(echo "$get_src" | $TAIDA /dev/stdin 2>/dev/null)
if echo "$get_output" | head -1 | grep -q "true" && echo "$get_output" | tail -1 | grep -q "false"; then
  pass "list.get() returns Lax"
else
  fail "list.get() returns Lax" "got: $get_output"
fi

# Div returns default on zero
div_fallback_src='
Div[10, 2]() ]=> r1
stdout(r1.toString())
Div[10, 0]() ]=> r2
stdout(r2.toString())
'
div_fallback_output=$(echo "$div_fallback_src" | $TAIDA /dev/stdin 2>/dev/null)
if echo "$div_fallback_output" | head -1 | grep -q "5" && echo "$div_fallback_output" | tail -1 | grep -q "0"; then
  pass "Div[x, y]() returns default on zero divisor"
else
  fail "Div[x, y]() returns default on zero divisor" "got: $div_fallback_output"
fi

echo ""

# =========================================================================
# Section 9: Semantic Parity Tests (Interpreter vs JS)
# =========================================================================
echo "=== Section 9: Semantic Parity Tests ==="

if command -v node >/dev/null 2>&1; then
  TMPDIR_SEM=$(mktemp -d)

  # Helper: run Taida source via interpreter, capture output or "THROW:<type>"
  sem_run_interp() {
    local src="$1"
    echo "$src" | $TAIDA /dev/stdin 2>/dev/null || true
  }

  # Helper: build JS from Taida source and run with node
  sem_run_js() {
    local src="$1"
    local jsf="$TMPDIR_SEM/sem_test_${RANDOM}_$$.js"
    if ! echo "$src" | $TAIDA build js /dev/stdin -o "$jsf" >/dev/null 2>&1; then
      echo "BUILD_ERROR"
      return
    fi
    node "$jsf" 2>/dev/null || true
  }

  # 9-1: Empty list first() returns Lax(hasValue=false)
  sem_src_first='
items: @[Int] <= @[]
lax <= items.first()
stdout(lax.hasValue.toString())
lax ]=> value
stdout(value.toString())
'
  interp_out=$(sem_run_interp "$sem_src_first")
  js_out=$(sem_run_js "$sem_src_first")
  if [ "$interp_out" = "$js_out" ]; then
    pass "semantic: empty list first() Lax fallback"
  else
    fail "semantic: empty list first() Lax fallback" "interp='$interp_out' js='$js_out'"
  fi

  # 9-2: Lax.hasValue as field access (Optional abolished, Lax is replacement)
  sem_src_hasvalue='
opt <= Lax[42]()
stdout(opt.hasValue().toString())
'
  interp_out=$(sem_run_interp "$sem_src_hasvalue")
  js_out=$(sem_run_js "$sem_src_hasvalue")
  if [ "$interp_out" = "$js_out" ]; then
    pass "semantic: Lax.hasValue field access"
  else
    fail "semantic: Lax.hasValue field access" "interp='$interp_out' js='$js_out'"
  fi

  # 9-3: Non-Bool predicate treated as false (strict === true)
  sem_src_pred='
items <= @[1, 2, 3]
result <= items.any(_ x = x)
stdout(result.toString())
'
  interp_out=$(sem_run_interp "$sem_src_pred")
  js_out=$(sem_run_js "$sem_src_pred")
  if [ "$interp_out" = "$js_out" ]; then
    pass "semantic: strict predicate (=== true)"
  else
    fail "semantic: strict predicate (=== true)" "interp='$interp_out' js='$js_out'"
  fi

  # 9-4: Structural equality in contains
  sem_src_struct='
items <= @[@(x <= 1, y <= 2)]
result <= items.contains(@(x <= 1, y <= 2))
stdout(result.toString())
'
  interp_out=$(sem_run_interp "$sem_src_struct")
  js_out=$(sem_run_js "$sem_src_struct")
  if [ "$interp_out" = "$js_out" ]; then
    pass "semantic: structural equality in contains"
  else
    fail "semantic: structural equality in contains" "interp='$interp_out' js='$js_out'"
  fi

  # 9-5: Empty list last() returns Lax(hasValue=false)
  sem_src_last='
items: @[Int] <= @[]
lax <= items.last()
stdout(lax.hasValue.toString())
lax ]=> value
stdout(value.toString())
'
  interp_out=$(sem_run_interp "$sem_src_last")
  js_out=$(sem_run_js "$sem_src_last")
  if [ "$interp_out" = "$js_out" ]; then
    pass "semantic: empty list last() Lax fallback"
  else
    fail "semantic: empty list last() Lax fallback" "interp='$interp_out' js='$js_out'"
  fi

  # 9-6: indexOf parity (structural equality)
  sem_src_indexof='
items <= @[@(a <= 1), @(a <= 2), @(a <= 3)]
idx <= items.indexOf(@(a <= 2))
stdout(idx.toString())
'
  interp_out=$(sem_run_interp "$sem_src_indexof")
  js_out=$(sem_run_js "$sem_src_indexof")
  if [ "$interp_out" = "$js_out" ]; then
    pass "semantic: indexOf structural equality"
  else
    fail "semantic: indexOf structural equality" "interp='$interp_out' js='$js_out'"
  fi

  # 9-7: unique parity
  sem_src_unique='
items <= @[1, 2, 2, 3, 1, 3]
result <= items.unique()
stdout(result.length().toString())
'
  interp_out=$(sem_run_interp "$sem_src_unique")
  js_out=$(sem_run_js "$sem_src_unique")
  if [ "$interp_out" = "$js_out" ]; then
    pass "semantic: unique parity"
  else
    fail "semantic: unique parity" "interp='$interp_out' js='$js_out'"
  fi

  # 9-8: BuchiPack order-independent equality
  sem_src_buchi_eq='
a <= @(x <= 1, y <= 2)
b <= @(y <= 2, x <= 1)
result <= a == b
stdout(result.toString())
'
  interp_out=$(sem_run_interp "$sem_src_buchi_eq")
  js_out=$(sem_run_js "$sem_src_buchi_eq")
  if [ "$interp_out" = "$js_out" ] && [ "$interp_out" = "true" ]; then
    pass "semantic: BuchiPack order-independent equality"
  else
    fail "semantic: BuchiPack order-independent equality" "interp='$interp_out' js='$js_out' (expected 'true')"
  fi

  # 9-9: Lax contains parity (Optional abolished)
  sem_src_lax_contains='
items <= @[Lax[1](), Lax[2]()]
result <= items.contains(Lax[1]())
stdout(result.toString())
'
  interp_out=$(sem_run_interp "$sem_src_lax_contains")
  js_out=$(sem_run_js "$sem_src_lax_contains")
  if [ "$interp_out" = "$js_out" ]; then
    pass "semantic: Lax contains parity"
  else
    fail "semantic: Lax contains parity" "interp='$interp_out' js='$js_out'"
  fi

  # 9-10: Result isSuccess/isError method parity
  sem_src_result='
Error => Fail = @(message: Str)
ok <= Result[42]()
err <= Result[0](throw <= Fail(message <= "fail"))
stdout(ok.isSuccess().toString())
stdout(ok.isError().toString())
stdout(err.isSuccess().toString())
stdout(err.isError().toString())
'
  interp_out=$(sem_run_interp "$sem_src_result")
  js_out=$(sem_run_js "$sem_src_result")
  if [ "$interp_out" = "$js_out" ]; then
    pass "semantic: Result isSuccess/isError parity"
  else
    fail "semantic: Result isSuccess/isError parity" "interp='$interp_out' js='$js_out'"
  fi

  rm -rf "$TMPDIR_SEM"
else
  skip "Semantic parity tests (node not available)"
fi

echo ""

# =========================================================================
# Section 10: Lax / Div / Mod / Type Conversion Tests (v0.5.0)
# =========================================================================
echo "=== Section 10: Lax / Div / Mod / Type Conversion ==="

TMPDIR_S10=$(mktemp -d)

# Helper: write source to temp file and run
s10_run() {
  local name="$1"
  local src="$2"
  local tmpf="$TMPDIR_S10/${name}.td"
  echo "$src" > "$tmpf"
  $TAIDA "$tmpf" 2>/dev/null
}

# 10-1: Div normal
div_normal_out=$(s10_run "div_normal" '
Div[10, 3]() ]=> result
stdout(result.toString())
')
if [ "$div_normal_out" = "3" ]; then
  pass "Div[10, 3]() => 3"
else
  fail "Div[10, 3]() => 3" "got: $div_normal_out"
fi

# 10-2: Div by zero returns default
div_zero_out=$(s10_run "div_zero" '
Div[10, 0]() ]=> result
stdout(result.toString())
')
if [ "$div_zero_out" = "0" ]; then
  pass "Div[10, 0]() => 0 (default)"
else
  fail "Div[10, 0]() => 0 (default)" "got: $div_zero_out"
fi

# 10-3: Div by zero hasValue is false
div_hv_out=$(s10_run "div_hv" '
stdout(Div[10, 0]().hasValue.toString())
')
if [ "$div_hv_out" = "false" ]; then
  pass "Div[10, 0]().hasValue => false"
else
  fail "Div[10, 0]().hasValue => false" "got: $div_hv_out"
fi

# 10-4: Mod normal
mod_normal_out=$(s10_run "mod_normal" '
Mod[10, 3]() ]=> result
stdout(result.toString())
')
if [ "$mod_normal_out" = "1" ]; then
  pass "Mod[10, 3]() => 1"
else
  fail "Mod[10, 3]() => 1" "got: $mod_normal_out"
fi

# 10-5: Mod by zero returns default
mod_zero_out=$(s10_run "mod_zero" '
Mod[10, 0]() ]=> result
stdout(result.toString())
')
if [ "$mod_zero_out" = "0" ]; then
  pass "Mod[10, 0]() => 0 (default)"
else
  fail "Mod[10, 0]() => 0 (default)" "got: $mod_zero_out"
fi

# 10-6: Int type conversion success
int_conv_out=$(s10_run "int_conv" '
Int["123"]() ]=> num
stdout(num.toString())
')
if [ "$int_conv_out" = "123" ]; then
  pass "Int[\"123\"]() => 123"
else
  fail "Int[\"123\"]() => 123" "got: $int_conv_out"
fi

# 10-7: Int type conversion failure
int_fail_out=$(s10_run "int_fail" '
Int["abc"]() ]=> num
stdout(num.toString())
')
if [ "$int_fail_out" = "0" ]; then
  pass "Int[\"abc\"]() => 0 (default)"
else
  fail "Int[\"abc\"]() => 0 (default)" "got: $int_fail_out"
fi

# 10-8: List get() returns Lax
list_get_out=$(s10_run "list_get" '
items <= @[10, 20, 30]
items.get(1) ]=> val
stdout(val.toString())
items.get(100) ]=> val2
stdout(val2.toString())
stdout(items.get(100).hasValue.toString())
')
expected_list_get=$(printf "20\n0\nfalse")
if [ "$list_get_out" = "$expected_list_get" ]; then
  pass "list.get() returns Lax"
else
  fail "list.get() returns Lax" "got: '$list_get_out' expected: '$expected_list_get'"
fi

# 10-9: first()/last() return Lax on empty list
first_last_out=$(s10_run "first_last" '
@[1, 2, 3].first() ]=> f
stdout(f.toString())
@[].first() ]=> f2
stdout(f2.toString())
stdout(@[].first().hasValue.toString())
@[1, 2, 3].last() ]=> l
stdout(l.toString())
@[].last() ]=> l2
stdout(l2.toString())
')
expected_fl=$(printf "1\n0\nfalse\n3\n0")
if [ "$first_last_out" = "$expected_fl" ]; then
  pass "first()/last() return Lax"
else
  fail "first()/last() return Lax" "got: '$first_last_out' expected: '$expected_fl'"
fi

# 10-10: max()/min() return Lax on empty list
max_min_out=$(s10_run "max_min" '
@[3, 1, 2].max() ]=> mx
stdout(mx.toString())
@[].max() ]=> mx2
stdout(mx2.toString())
@[3, 1, 2].min() ]=> mn
stdout(mn.toString())
@[].min() ]=> mn2
stdout(mn2.toString())
')
expected_mm=$(printf "3\n0\n1\n0")
if [ "$max_min_out" = "$expected_mm" ]; then
  pass "max()/min() return Lax"
else
  fail "max()/min() return Lax" "got: '$max_min_out' expected: '$expected_mm'"
fi

rm -rf "$TMPDIR_S10"
echo ""

# =========================================================================
# Section 11: JS Lax / Div / Mod Parity (v0.5.0)
# =========================================================================
echo "=== Section 11: JS Lax / Div / Mod Parity ==="

if command -v node >/dev/null 2>&1; then
  TMPDIR_LAX=$(mktemp -d)

  # Helper: write source to temp file, run with interpreter
  lax_run_interp() {
    local tmpf="$TMPDIR_LAX/interp_$RANDOM.td"
    echo "$1" > "$tmpf"
    $TAIDA "$tmpf" 2>/dev/null || true
  }

  # Helper: write source to temp file, build JS and run with node
  lax_run_js() {
    local tmpf="$TMPDIR_LAX/js_$RANDOM.td"
    local jsf="$TMPDIR_LAX/js_$RANDOM.js"
    echo "$1" > "$tmpf"
    if ! $TAIDA build js "$tmpf" -o "$jsf" >/dev/null 2>&1; then
      echo "BUILD_ERROR"
      return
    fi
    node "$jsf" 2>/dev/null || true
  }

  # 11-1: Div parity
  lax_div_src='
Div[10, 3]() ]=> r
stdout(r.toString())
Div[10, 0]() ]=> r2
stdout(r2.toString())
stdout(Div[10, 0]().hasValue.toString())
'
  interp_out=$(lax_run_interp "$lax_div_src")
  js_out=$(lax_run_js "$lax_div_src")
  if [ "$interp_out" = "$js_out" ]; then
    pass "JS parity: Div mold"
  else
    fail "JS parity: Div mold" "interp='$interp_out' js='$js_out'"
  fi

  # 11-2: Mod parity
  lax_mod_src='
Mod[10, 3]() ]=> r
stdout(r.toString())
Mod[10, 0]() ]=> r2
stdout(r2.toString())
'
  interp_out=$(lax_run_interp "$lax_mod_src")
  js_out=$(lax_run_js "$lax_mod_src")
  if [ "$interp_out" = "$js_out" ]; then
    pass "JS parity: Mod mold"
  else
    fail "JS parity: Mod mold" "interp='$interp_out' js='$js_out'"
  fi

  # 11-3: Int type conversion parity
  lax_int_src='
Int["42"]() ]=> r
stdout(r.toString())
Int["abc"]() ]=> r2
stdout(r2.toString())
'
  interp_out=$(lax_run_interp "$lax_int_src")
  js_out=$(lax_run_js "$lax_int_src")
  if [ "$interp_out" = "$js_out" ]; then
    pass "JS parity: Int[] type conversion"
  else
    fail "JS parity: Int[] type conversion" "interp='$interp_out' js='$js_out'"
  fi

  # 11-4: list.get() Lax parity
  lax_get_src='
items <= @[10, 20, 30]
items.get(1) ]=> v
stdout(v.toString())
items.get(100) ]=> v2
stdout(v2.toString())
stdout(items.get(100).hasValue.toString())
'
  interp_out=$(lax_run_interp "$lax_get_src")
  js_out=$(lax_run_js "$lax_get_src")
  if [ "$interp_out" = "$js_out" ]; then
    pass "JS parity: list.get() Lax"
  else
    fail "JS parity: list.get() Lax" "interp='$interp_out' js='$js_out'"
  fi

  rm -rf "$TMPDIR_LAX"
else
  skip "JS Lax parity tests (node not available)"
fi

echo ""

# =========================================================================
# Summary
# =========================================================================
echo "========================================="
echo "Results: $PASS passed, $FAIL failed, $SKIP skipped"
echo "========================================="

# Explain common skip reasons:
# Tests are skipped for the following reasons:
#   1. Examples with no expected output file in tests/expected/
#      are skipped by Section 1 (output comparison).
#   2. Some JS build examples may be skipped when the interpreter
#      fails and comparison is not possible.
# Note: std/math, std/time, std/random, std/env, std/process, std/http
# modules were dissolved. stdout/jsonParse/etc. are now prelude builtins.
if [ "$SKIP" -gt 0 ]; then
  echo ""
  echo "Skipped tests: examples without expected output files in tests/expected/."
fi

if [ -n "$ERRORS" ]; then
  echo ""
  echo "=== Failure Details ==="
  printf '%s' "$ERRORS"
fi

if [ "$FAIL" -gt 0 ]; then
  exit 1
fi

exit 0
