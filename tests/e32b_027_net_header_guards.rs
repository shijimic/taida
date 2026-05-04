//! E32B-027: streaming response headers must reject CR/LF injection.

const INTERP_TYPES: &str = include_str!("../src/interpreter/net_eval/types.rs");
const JS_NET: &str = include_str!("../src/js/runtime/net.rs");
const NATIVE_NET: &str = include_str!("../src/codegen/native_runtime/net_h1_h2.c");

#[test]
fn e32b_027_streaming_crlf_guards_are_installed() {
    assert!(
        INTERP_TYPES.contains("startResponse: headers[{}].name contains CR/LF")
            && INTERP_TYPES.contains("startResponse: headers[{}].value contains CR/LF"),
        "interpreter startResponse must reject CR/LF in streaming response headers"
    );

    assert!(
        JS_NET.contains("function __taida_net_validateStreamingHeaders(headers)")
            && JS_NET.contains("__taida_net_validateStreamingHeaders(h);")
            && JS_NET.contains("startResponse: headers[' + i + '].name contains CR/LF")
            && JS_NET.contains("startResponse: headers[' + i + '].value contains CR/LF"),
        "JS startResponse must reject CR/LF before staging streaming response headers"
    );

    assert!(
        NATIVE_NET.contains("static int taida_net3_validate_streaming_headers")
            && NATIVE_NET
                .contains("taida_net3_validate_streaming_headers(headers, \"startResponse\")")
            && NATIVE_NET.contains("%s: headers[%d].name contains CR/LF")
            && NATIVE_NET.contains("%s: headers[%d].value contains CR/LF"),
        "native startResponse must reject CR/LF before staging streaming response headers"
    );
}
