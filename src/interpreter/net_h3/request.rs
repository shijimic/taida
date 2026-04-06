/// H3 pseudo-header extraction, request validation, response builders.
///
/// This module contains:
/// - H3RequestError, H3RequestFields
/// - extract_request_fields() — pseudo-header validation matching H2 semantics
/// - build_response_headers_frame() / build_data_frame()
/// - Selftests mirroring Native reference

use super::qpack::{
    qpack_encode_block, qpack_decode_block, H3Header,
};
use super::frame::{
    encode_frame,
    H3_FRAME_HEADERS, H3_FRAME_DATA,
};


// ── H3 Request Extraction ────────────────────────────────────────────────
// Mirrors h3_extract_request_fields in native_runtime.c.
// Validates pseudo-headers matching H2 semantics (NB7-10).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum H3RequestError {
    Ordering,
    UnknownPseudo,
    MissingPseudo,
    DuplicatePseudo,
    EmptyPseudo,
}

/// Extracted request fields from H3 pseudo-headers.
#[derive(Debug)]
pub(crate) struct H3RequestFields {
    pub method: String,
    pub path: String,
    pub authority: String,
    pub regular_headers: Vec<(String, String)>,
}

/// Extract request fields from decoded H3 headers.
/// Validates pseudo-header ordering, duplicates, required fields, and empty values.
/// Returns the extracted fields on success, or an error kind.
pub(crate) fn extract_request_fields(
    headers: &[H3Header],
) -> Result<H3RequestFields, H3RequestError> {
    let mut method = None;
    let mut path = None;
    let mut authority = None;
    let mut scheme = None;
    let mut saw_regular = false;
    let mut saw_method = false;
    let mut saw_path = false;
    let mut saw_authority = false;
    let mut saw_scheme = false;
    let mut regular_headers = Vec::new();

    for hdr in headers {
        if hdr.name.starts_with(':') {
            if saw_regular {
                return Err(H3RequestError::Ordering);
            }
            match hdr.name.as_str() {
                ":method" => {
                    if saw_method {
                        return Err(H3RequestError::DuplicatePseudo);
                    }
                    saw_method = true;
                    method = Some(hdr.value.clone());
                }
                ":path" => {
                    if saw_path {
                        return Err(H3RequestError::DuplicatePseudo);
                    }
                    saw_path = true;
                    path = Some(hdr.value.clone());
                }
                ":authority" => {
                    if saw_authority {
                        return Err(H3RequestError::DuplicatePseudo);
                    }
                    saw_authority = true;
                    authority = Some(hdr.value.clone());
                }
                ":scheme" => {
                    if saw_scheme {
                        return Err(H3RequestError::DuplicatePseudo);
                    }
                    saw_scheme = true;
                    scheme = Some(hdr.value.clone());
                }
                _ => {
                    return Err(H3RequestError::UnknownPseudo);
                }
            }
        } else {
            saw_regular = true;
            regular_headers.push((hdr.name.clone(), hdr.value.clone()));
        }
    }

    // Required pseudo-headers: :method, :path, :scheme (matches H2 semantics)
    if !saw_method || !saw_path || !saw_scheme {
        return Err(H3RequestError::MissingPseudo);
    }

    let method = method.unwrap();
    let path = path.unwrap();
    let scheme = scheme.unwrap();
    let authority = authority.unwrap_or_default();
    // NB7-29: :authority is conditionally required per RFC 9114 §4.1.
    // Empty value is valid (matches H2 behavior). No deviation from h1/h2 compatibility policy.

    // Reject empty pseudo-header values (matches H2 semantics)
    if method.is_empty() || path.is_empty() || scheme.is_empty() {
        return Err(H3RequestError::EmptyPseudo);
    }

    Ok(H3RequestFields {
        method,
        path,
        authority,
        regular_headers,
    })
}

// ── H3 Response Builders ─────────────────────────────────────────────────

/// Build an H3 HEADERS frame with QPACK-encoded response headers.
pub(crate) fn build_response_headers_frame(
    status: u16,
    headers: &[(String, String)],
) -> Option<Vec<u8>> {
    let qpack_block = qpack_encode_block(status, headers)?;
    encode_frame(H3_FRAME_HEADERS, &qpack_block)
}

/// Build an H3 DATA frame.
pub(crate) fn build_data_frame(data: &[u8]) -> Option<Vec<u8>> {
    encode_frame(H3_FRAME_DATA, data)
}

// ── H3 Self-Tests ────────────────────────────────────────────────────────
// Mirrors the Native self-tests (NB7-9, NB7-10, NB7-11) for parity.

/// Result of self-test execution.
#[derive(Debug)]
pub(crate) enum SelftestResult {
    Ok,
    QpackFailure(i32),
    ValidationFailure(i32),
}

/// Run all H3 self-tests. Returns `SelftestResult::Ok` if all pass.
pub(crate) fn run_selftests() -> SelftestResult {
    match selftest_qpack_roundtrip() {
        0 => {}
        rc => return SelftestResult::QpackFailure(rc),
    }
    match selftest_request_validation() {
        0 => {}
        rc => return SelftestResult::ValidationFailure(rc),
    }
    SelftestResult::Ok
}

/// NB7-9: QPACK encode/decode round-trip self-test.
pub(crate) fn selftest_qpack_roundtrip() -> i32 {
    // Encode a response with 4 custom headers
    let headers = vec![
        ("content-type".to_string(), "text/plain".to_string()),
        ("x-custom-header".to_string(), "custom-value-123".to_string()),
        ("x-request-id".to_string(), "abc-def-ghi".to_string()),
        ("accept".to_string(), "application/json".to_string()),
    ];

    // Encode
    let encoded = match qpack_encode_block(200, &headers) {
        Some(e) => e,
        None => return -1,
    };

    // Decode
    let decoded = match qpack_decode_block(&encoded, 8, None, None) {
        Some(d) => d,
        None => return -2,
    };

    // Expected: :status + 4 headers = 5
    if decoded.len() != 5 {
        return -2;
    }

    // Verify :status
    if decoded[0].name != ":status" {
        return -3;
    }
    if decoded[0].value != "200" {
        return -4;
    }

    // Verify round-trip for each input header
    for (i, (name, value)) in headers.iter().enumerate() {
        if decoded[i + 1].name != *name {
            return -(10 + i as i32);
        }
        if decoded[i + 1].value != *value {
            return -(20 + i as i32);
        }
    }

    // NB7-11: Test max_headers overflow
    match qpack_decode_block(&encoded, 2, None, None) {
        None => {} // correct: overflow = decode error
        Some(_) => return -30,
    }

    0
}

/// NB7-10: H3 request pseudo-header validation self-test.
pub(crate) fn selftest_request_validation() -> i32 {
    // Test 1: Valid request
    {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
            H3Header { name: ":scheme".into(), value: "https".into() },
            H3Header { name: ":authority".into(), value: "localhost".into() },
        ];
        if extract_request_fields(&hdrs).is_err() {
            return -1;
        }
    }

    // Test 2: Missing :scheme should fail
    {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
        ];
        match extract_request_fields(&hdrs) {
            Err(H3RequestError::MissingPseudo) => {}
            _ => return -2,
        }
    }

    // Test 3: Empty :scheme should fail
    {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
            H3Header { name: ":scheme".into(), value: "".into() },
        ];
        match extract_request_fields(&hdrs) {
            Err(H3RequestError::EmptyPseudo) => {}
            _ => return -4,
        }
    }

    // Test 4: Empty :method should fail
    {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "".into() },
            H3Header { name: ":path".into(), value: "/".into() },
            H3Header { name: ":scheme".into(), value: "https".into() },
        ];
        match extract_request_fields(&hdrs) {
            Err(H3RequestError::EmptyPseudo) => {}
            _ => return -6,
        }
    }

    // Test 5: Duplicate :scheme should fail
    {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
            H3Header { name: ":scheme".into(), value: "https".into() },
            H3Header { name: ":scheme".into(), value: "http".into() },
        ];
        match extract_request_fields(&hdrs) {
            Err(H3RequestError::DuplicatePseudo) => {}
            _ => return -8,
        }
    }

    // Test 6: Ordering violation (regular before pseudo)
    {
        let hdrs = vec![
            H3Header { name: "host".into(), value: "localhost".into() },
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
        ];
        match extract_request_fields(&hdrs) {
            Err(H3RequestError::Ordering) => {}
            _ => return -10,
        }
    }

    // Test 7: Unknown pseudo-header should fail
    {
        let hdrs = vec![
            H3Header { name: ":method".into(), value: "GET".into() },
            H3Header { name: ":path".into(), value: "/".into() },
            H3Header { name: ":scheme".into(), value: "https".into() },
            H3Header { name: ":protocol".into(), value: "websocket".into() },
        ];
        match extract_request_fields(&hdrs) {
            Err(H3RequestError::UnknownPseudo) => {}
            _ => return -12,
        }
    }

    0
}
