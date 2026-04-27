#!/usr/bin/env bash
#
# D29B-013 / Lock-I — release.yml idempotent dispatcher unit test
#
# # Why this test exists
#
# `.github/workflows/release.yml` previously used a bare `gh release
# create "${RELEASE_TAG}" dist/* ...` invocation that hard-failed with
# `release already exists` whenever the tag had been published earlier.
# The `@d.28` release run hit this and required a manual
# `gh release delete` + workflow rerun (operator intervention is a
# stable-cycle contract violation).
#
# Lock-I (Phase 0 verdict) rewrote that step into an existence-check
# dispatcher:
#
#   if gh release view "${RELEASE_TAG}" --repo "${REPO}" >/dev/null 2>&1; then
#     gh release upload "${RELEASE_TAG}" dist/* --repo "${REPO}" --clobber
#     gh release edit   "${RELEASE_TAG}"        --repo "${REPO}" --notes ...
#   else
#     gh release create "${RELEASE_TAG}" dist/* --repo "${REPO}" --notes ...
#   fi
#
# This script extracts that dispatcher logic, mocks `gh` with a shell
# function, and asserts that:
#
#   1. When `gh release view` returns 0 (release exists), the script
#      invokes `gh release upload --clobber` and `gh release edit`,
#      and never invokes `gh release create`.
#
#   2. When `gh release view` returns 1 (release absent), the script
#      invokes `gh release create`, and never invokes `gh release upload`
#      or `gh release edit`.
#
# # Acceptance
#
# `bash tests/d29b_013_release_idempotency.sh` exits 0.
#
# Note: this is a Bash unit test, not a `cargo test`. It is invoked
# directly from CI in the same job that runs `actionlint` (so the test
# set is "static checks for .github/"). Running under `cargo test` would
# require shelling out, which adds no isolation value.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RELEASE_YML="${REPO_ROOT}/.github/workflows/release.yml"

if [[ ! -f "${RELEASE_YML}" ]]; then
  echo "FATAL: release.yml not found at ${RELEASE_YML}" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Extracted dispatcher under test
# ---------------------------------------------------------------------------
#
# This is a verbatim copy of the Lock-I dispatcher logic. If release.yml
# diverges from this snippet, both this test and the
# `d29b_010_release_yml_has_idempotent_dispatcher` cargo test will catch
# the regression (cargo test = grep for sentinels; this test = behavioural
# pin).
dispatcher() {
  local RELEASE_TAG="$1"
  local REPO="$2"
  local NOTES="$3"

  if gh release view "${RELEASE_TAG}" --repo "${REPO}" >/dev/null 2>&1; then
    echo "release ${RELEASE_TAG} already exists; updating assets and notes"
    gh release upload "${RELEASE_TAG}" dist/* \
      --repo "${REPO}" \
      --clobber
    gh release edit "${RELEASE_TAG}" \
      --repo "${REPO}" \
      --title "${RELEASE_TAG}" \
      --notes "${NOTES}"
  else
    echo "release ${RELEASE_TAG} does not exist; creating"
    gh release create "${RELEASE_TAG}" dist/* \
      --repo "${REPO}" \
      --title "${RELEASE_TAG}" \
      --notes "${NOTES}"
  fi
}

# ---------------------------------------------------------------------------
# Test harness — mock `gh` and capture invocation log
# ---------------------------------------------------------------------------

# Each test runs in its own subshell so the global state (gh mock script,
# captured log) is isolated.

run_case() {
  local case_name="$1"
  local view_exit="$2"   # 0 = release exists, 1 = release absent
  shift 2
  local expected_subcommands=("$@")  # e.g. ("upload" "edit") or ("create")

  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "${tmp}"' RETURN

  local log="${tmp}/gh_log"
  : >"${log}"

  # Build a mock `gh` shim and prepend it to PATH.
  cat >"${tmp}/gh" <<EOF
#!/usr/bin/env bash
echo "\$@" >>"${log}"
case "\$1 \$2" in
  "release view")
    exit ${view_exit}
    ;;
  "release upload"|"release edit"|"release create")
    exit 0
    ;;
  *)
    exit 0
    ;;
esac
EOF
  chmod +x "${tmp}/gh"
  PATH="${tmp}:${PATH}" dispatcher "@d.test" "owner/repo" "release notes" >/dev/null

  local subcommands_seen
  subcommands_seen="$(awk '/^release / {print $2}' "${log}" | sort -u | tr '\n' ' ')"
  local subcommands_expected
  subcommands_expected="$(printf '%s\n' "${expected_subcommands[@]}" view | sort -u | tr '\n' ' ')"

  if [[ "${subcommands_seen}" != "${subcommands_expected}" ]]; then
    echo "FAIL [${case_name}]:"
    echo "  expected gh release subcommands: ${subcommands_expected}"
    echo "  observed gh release subcommands: ${subcommands_seen}"
    echo "  full mock log:"
    sed 's/^/    /' "${log}"
    return 1
  fi

  # Also assert the forbidden subcommands are absent.
  case "${case_name}" in
    "release exists")
      if grep -q '^release create' "${log}"; then
        echo "FAIL [${case_name}]: dispatcher must NOT call \`gh release create\` when the release exists"
        sed 's/^/    /' "${log}"
        return 1
      fi
      ;;
    "release absent")
      if grep -qE '^release (upload|edit)' "${log}"; then
        echo "FAIL [${case_name}]: dispatcher must NOT call \`gh release upload\` or \`gh release edit\` when the release is absent"
        sed 's/^/    /' "${log}"
        return 1
      fi
      ;;
  esac

  echo "PASS [${case_name}]"
  return 0
}

failures=0

if ! run_case "release exists" 0 "upload" "edit"; then
  failures=$((failures + 1))
fi

if ! run_case "release absent" 1 "create"; then
  failures=$((failures + 1))
fi

echo "----------------------------------------"
if [[ "${failures}" -gt 0 ]]; then
  echo "D29B-013 / Lock-I: ${failures} case(s) failed"
  exit 1
fi
echo "D29B-013 / Lock-I: all idempotency cases pass"
exit 0
