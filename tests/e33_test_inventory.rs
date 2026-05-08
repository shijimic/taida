//! E33 test-inventory policy pins for ignored and environment-dependent tests.

const NEXTEST: &str = include_str!("../.config/nextest.toml");
const CI: &str = include_str!("../.github/workflows/ci.yml");
const DOC_REFRESH: &str = include_str!("../.github/workflows/doc-baseline-refresh.yml");

#[test]
fn environment_dependent_tests_are_grouped_in_nextest() {
    for group in ["external-backend", "wasm-runtime", "long-running-ignored"] {
        assert!(
            NEXTEST.contains(group),
            "nextest config must define the `{group}` test group"
        );
    }
    for binary in [
        "js_execution",
        "wasm_min",
        "wasm_wasi",
        "wasm_full",
        "c27b_003_portbind_race",
        "c26b_024_router_bench_parity",
    ] {
        assert!(
            NEXTEST.contains(binary),
            "nextest external/ignored inventory must include `{binary}`"
        );
    }
}

#[test]
fn ci_documents_optional_ignored_test_inventory() {
    assert!(
        CI.contains("Ignored / optional test inventory")
            && CI.contains("cargo nextest list --profile ci --run-ignored only"),
        "ci.yml must expose the ignored/optional inventory listing command"
    );
}

#[test]
fn doc_baseline_refresh_workflow_runs_probe_helper() {
    assert!(
        DOC_REFRESH.contains("workflow_dispatch:")
            && DOC_REFRESH.contains("c25b_008_doc_examples_probe")
            && DOC_REFRESH.contains("--ignored --nocapture"),
        "doc-baseline-refresh workflow must run the ignored probe helper on demand"
    );
}
