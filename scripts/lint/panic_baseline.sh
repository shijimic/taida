#!/usr/bin/env bash
# scripts/lint/panic_baseline.sh
#
# D28B-020 (Option A) — production `panic!` baseline pin / forward-protection gate.
#
# Counts `panic!` occurrences in production regions of `src/**/*.rs` and rejects
# any drift from the pinned baseline. "Production region" is defined as the
# portion of each file *before* the first `#[cfg(test)]` line, with whole-file
# exclusion applied to test-only files (filename matches `*_tests.rs` or
# `tests.rs`, which are included via `#[cfg(test)] mod tests;` from a sibling
# module file and therefore have no top-level `#[cfg(test)]` attribute of their
# own).
#
# Baseline rationale (2026-04-26 audit, Round 1 wB):
#   The contract initially assumed production `panic!` count was 0. Direct grep
#   over `src/graph/` and `src/addon/` confirmed 0 there, but a full-tree audit
#   discovered 2 invariant-violation `BUG:` panics in unrelated modules:
#     - src/codegen/driver.rs (IR-cache invariant in incremental fuse path)
#     - src/parser/ast.rs     (`body_expr()` precondition on AST arm)
#   These are intentional internal-invariant panics (precondition violations
#   that signal a compiler bug, not user-input failures), allowed by D28B-020's
#   "invariant 違反の internal panic は限定的に許容され得る" judgment. They
#   are pinned by the ALLOWLIST below and counted toward the baseline. Any
#   change to count or to the allowlisted file:line set fails this gate.
#
# Exit codes:
#   0 — production panic! count matches baseline AND every observed site is in
#       the allowlist
#   1 — drift detected (count mismatch, new site, or removed site not yet
#       reflected in the allowlist)
#
# Usage:
#   bash scripts/lint/panic_baseline.sh
#
# Maintenance:
#   To remove an entry from the allowlist after refactoring a `panic!` away,
#   delete the matching `file:line` from PANIC_BASELINE and decrement
#   PANIC_BASELINE_COUNT in lockstep. To add a new allowed invariant panic,
#   require D28 driver / user verdict — this gate is meant to *prevent* silent
#   addition.

set -euo pipefail

# --- Pinned baseline -----------------------------------------------------------
# Each entry is `path:line` relative to the repo root, sorted ascending.
# Update only with explicit user verdict (D28B-020 maintenance entry).
PANIC_BASELINE=(
  "src/codegen/driver.rs:1228"
  "src/parser/ast.rs:424"
)
PANIC_BASELINE_COUNT=${#PANIC_BASELINE[@]}

# --- Helpers -----------------------------------------------------------------
# Print "path:line" for every production `panic!` occurrence in src/**/*.rs.
collect_production_panics() {
  # Locate src/ relative to this script so the gate works from any CWD.
  local script_dir
  script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  local repo_root
  repo_root="$(cd "${script_dir}/../.." && pwd)"

  # Iterate all .rs files under src/, skip whole-file test-only modules,
  # truncate at the first `#[cfg(test)]` line, and emit `path:line` for each
  # `panic!` token.
  while IFS= read -r f; do
    local base
    base="$(basename "$f")"
    case "$base" in
      *_tests.rs|tests.rs)
        # Whole file is a test module included via `#[cfg(test)] mod tests;`.
        continue
        ;;
    esac

    awk -v path="${f#${repo_root}/}" '
      /^#\[cfg\(test\)\]/ { exit }
      /panic!/            { printf "%s:%d\n", path, NR }
    ' "$f"
  done < <(find "${repo_root}/src" -type f -name '*.rs' | sort)
}

# --- Main --------------------------------------------------------------------
mapfile -t observed < <(collect_production_panics | sort)
observed_count=${#observed[@]}

# Fast path: count matches and every observed entry is in the baseline.
status=0

# Build a lookup table for the baseline.
declare -A baseline_set=()
for entry in "${PANIC_BASELINE[@]}"; do
  baseline_set["$entry"]=1
done

# Detect new sites (in observed, not in baseline).
new_sites=()
for entry in "${observed[@]}"; do
  if [[ -z "${baseline_set[$entry]:-}" ]]; then
    new_sites+=("$entry")
  fi
done

# Detect removed sites (in baseline, not in observed).
declare -A observed_set=()
for entry in "${observed[@]}"; do
  observed_set["$entry"]=1
done
removed_sites=()
for entry in "${PANIC_BASELINE[@]}"; do
  if [[ -z "${observed_set[$entry]:-}" ]]; then
    removed_sites+=("$entry")
  fi
done

# Report.
echo "panic_baseline.sh — D28B-020 production panic! gate"
echo "  baseline count : ${PANIC_BASELINE_COUNT}"
echo "  observed count : ${observed_count}"

if [[ "${observed_count}" -ne "${PANIC_BASELINE_COUNT}" ]]; then
  status=1
  echo "  count drift    : observed=${observed_count} baseline=${PANIC_BASELINE_COUNT}" >&2
fi

if [[ "${#new_sites[@]}" -gt 0 ]]; then
  status=1
  echo "  new panic! sites (must be reviewed before adding to baseline):" >&2
  for entry in "${new_sites[@]}"; do
    echo "    + $entry" >&2
  done
fi

if [[ "${#removed_sites[@]}" -gt 0 ]]; then
  status=1
  echo "  removed panic! sites (baseline must be updated in lockstep):" >&2
  for entry in "${removed_sites[@]}"; do
    echo "    - $entry" >&2
  done
fi

if [[ "${status}" -eq 0 ]]; then
  echo "  result         : OK (production panic! count and sites match baseline)"
else
  echo "  result         : DRIFT — fix or update PANIC_BASELINE in lockstep" >&2
fi

exit "${status}"
