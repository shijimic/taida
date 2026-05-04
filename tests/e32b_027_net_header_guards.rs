//! E32B-027: streaming response headers must reject unsafe header input.

const INTERP_TYPES: &str = include_str!("../src/interpreter/net_eval/types.rs");
const INTERP_STREAM: &str = include_str!("../src/interpreter/net_eval/stream.rs");
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

#[test]
fn e32b_027_streaming_length_and_shape_guards_are_installed() {
    assert!(
        INTERP_TYPES.contains("STREAMING_HEADER_NAME_MAX_BYTES: usize = 8192")
            && INTERP_TYPES.contains("STREAMING_HEADER_VALUE_MAX_BYTES: usize = 65536")
            && INTERP_TYPES.contains("startResponse: headers[{}].name exceeds {} bytes")
            && INTERP_TYPES.contains("startResponse: headers[{}].value exceeds {} bytes"),
        "interpreter startResponse must enforce streaming header byte limits"
    );

    assert!(
        INTERP_STREAM.contains("startResponse: headers must be a List, got {}")
            && INTERP_STREAM.contains("startResponse: headers[{}] must be @(name, value)")
            && INTERP_STREAM.contains("startResponse: headers[{}].name must be Str")
            && INTERP_STREAM.contains("startResponse: headers[{}].value must be Str"),
        "interpreter startResponse must reject shape mismatches"
    );

    assert!(
        JS_NET.contains("startResponse: headers must be a List, got ")
            && JS_NET.contains("startResponse: headers[' + i + '] must be @(name, value)")
            && JS_NET.contains("startResponse: headers[' + i + '].name must be Str")
            && JS_NET.contains("startResponse: headers[' + i + '].value must be Str")
            && JS_NET.contains("Buffer.byteLength(name, 'utf-8') > 8192")
            && JS_NET.contains("Buffer.byteLength(value, 'utf-8') > 65536"),
        "JS startResponse must reject shape mismatches and oversize streaming headers"
    );

    assert!(
        NATIVE_NET.contains("%s: headers must be a List")
            && NATIVE_NET.contains("%s: headers[%d] must be @(name, value)")
            && NATIVE_NET.contains("%s: headers[%d].name must be Str")
            && NATIVE_NET.contains("%s: headers[%d].value must be Str")
            && NATIVE_NET.contains("%s: headers[%d].name exceeds 8192 bytes")
            && NATIVE_NET.contains("%s: headers[%d].value exceeds 65536 bytes"),
        "native startResponse must reject shape mismatches and oversize streaming headers"
    );
}
