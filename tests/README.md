# Test Binary Resolution

Shell-based integration tests resolve `taida` through `tests/scripts/lib_taida_bin.sh`.

Use `TAIDA_BIN` when a job needs a specific binary:

```sh
TAIDA_BIN="$PWD/target/release/taida" ./tests/e2e_smoke.sh
```

`TAIDA_BIN` may be relative to the caller's current directory, but CI should
prefer absolute paths so scripts that change directories keep invoking the same
binary. When `TAIDA_BIN` is not set, the helper looks for
`target/release/taida` and then `target/debug/taida` under the repository root.
Scripts that are expected to run from a clean checkout, such as
`tests/e2e_smoke.sh` and `tests/run_backend_parity.sh`, build `taida` first
when neither candidate exists.

Rust integration tests use `tests/common::taida_bin()`, which follows the same
runtime environment override and avoids compile-time binary paths. That matters
for archive-based test execution, where tests may run in a different checkout
layout from the one that compiled them.
