# C26B-003: port-bind race recurrence fixture

Exercises `httpServe` on an explicit loopback port and verifies it binds
cleanly on all 3 backends (Interpreter / JS / Native). Serves as a sanity
fixture for the Phase 3 root-cause fix documented in
`.dev/C26B_003_ANALYSIS.md`.

## Files

- `serve_one.td` — minimal one-shot server. Ports are assigned by the
  external test harness (see `tests/parity.rs::c26b_003_*`), not by the
  fixture itself, because the race being guarded is on the **allocator**
  side, not the Taida-language side.

## Usage

Stand-alone smoke test (manual):

```bash
# Interpreter
TAIDA_PORT=18080 taida examples/quality/c26_portbind/serve_one.td &
sleep 0.5
curl http://127.0.0.1:18080/ ; kill %1

# JS
TAIDA_PORT=18081 taida build --target js examples/quality/c26_portbind/serve_one.td -o /tmp/c26_portbind.mjs
node /tmp/c26_portbind.mjs &
sleep 0.5
curl http://127.0.0.1:18081/ ; kill %1

# Native
TAIDA_PORT=18082 taida build --target native examples/quality/c26_portbind/serve_one.td -o /tmp/c26_portbind.bin
/tmp/c26_portbind.bin &
sleep 0.5
curl http://127.0.0.1:18082/ ; kill %1
```

All 3 backends must print `ok=true requests=1` to stdout.

## Acceptance (C26B-003)

The race being guarded lives in the test harness allocator
(`find_free_loopback_port` in `tests/parity.rs`). The Rust-side guards
`c26b_003_allocator_stays_below_ephemeral_range`,
`c26b_003_handoff_race_20x_concurrent_children`, and
`c26b_003_sequential_100x_allocate_then_bind` enforce the invariants.

This fixture is provided for 3-backend parity verification when an
operator wants to manually confirm that `httpServe` on an explicit port
works across all backends — the Taida-language surface itself is
unchanged and the fix is entirely test-harness-local.
