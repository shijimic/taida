//! E33 JS Error field-lift regression pins.

const JS_CORE: &str = include_str!("../src/js/runtime/core.rs");
const JS_NET: &str = include_str!("../src/js/runtime/net.rs");

#[test]
fn js_taida_error_lifts_fields_to_top_level() {
    assert!(
        JS_CORE.contains("for (const k of Object.keys(fields))"),
        "__TaidaError must iterate fields for top-level lift"
    );
    assert!(
        JS_CORE.contains("if (k === 'type' || k === 'message' || k === 'name' || k === 'stack')"),
        "__TaidaError must preserve standard Error properties while lifting custom fields"
    );
    assert!(
        JS_CORE.contains("this[k] = fields[k];"),
        "__TaidaError must expose custom fields such as kind/code on the caught error"
    );
}

#[test]
fn js_result_throw_forwards_payload_fields() {
    assert!(
        JS_CORE.contains("new __TaidaError(_throw.type || 'ResultError', _throw.message || String(_throw), _throw)"),
        "Result getOrThrow/unmold must forward the throw payload as fields"
    );
}

#[test]
fn js_net_result_failure_keeps_kind_on_top_level_and_fields() {
    assert!(
        JS_NET.contains("kind: kind, fields: { kind: kind }"),
        "net Result failures must expose kind both directly and through fields"
    );
}
