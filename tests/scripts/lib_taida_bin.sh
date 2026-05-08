#!/usr/bin/env bash
# Shared taida binary resolution for shell-based integration tests.

taida_test_repo_root() {
  local lib_dir
  lib_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
  (cd "$lib_dir/../.." && pwd -P)
}

taida_abs_path() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s\n' "$(pwd -P)/$1" ;;
  esac
}

resolve_taida_bin() {
  local repo_root candidate

  if [ -n "${TAIDA_BIN:-}" ]; then
    candidate="$(taida_abs_path "$TAIDA_BIN")"
    if [ -x "$candidate" ]; then
      printf '%s\n' "$candidate"
      return 0
    fi
    echo "TAIDA_BIN is set but is not executable: $TAIDA_BIN" >&2
    echo "resolved path: $candidate" >&2
    return 1
  fi

  repo_root="$(taida_test_repo_root)"
  for candidate in "$repo_root/target/release/taida" "$repo_root/target/debug/taida"; do
    if [ -x "$candidate" ]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  echo "TAIDA_BIN is not set and no built taida binary exists under target/{release,debug}/taida" >&2
  echo "Run 'cargo build --bin taida' or set TAIDA_BIN to an executable absolute path." >&2
  return 1
}
