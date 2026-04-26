// ── taida-lang/net: HTTP v1 runtime ─────────────────────────────
// httpParseRequestHead, httpEncodeResponse, httpServe
// These are dedicated net runtime functions, not os wrappers.

// Forward declarations
taida_val taida_net_http_parse_request_head(taida_val input);
taida_val taida_net_http_encode_response(taida_val response);
taida_val taida_net_http_serve(taida_val port, taida_val handler, taida_val max_requests, taida_val timeout_ms, taida_val max_connections, taida_val tls, taida_val handler_type_tag, taida_val handler_arity);
taida_val taida_net_read_body(taida_val req);
// NET3-5b: v3 streaming API forward declarations
taida_val taida_net_start_response(taida_val writer, taida_val status, taida_val headers);
taida_val taida_net_write_chunk(taida_val writer, taida_val data);
taida_val taida_net_end_response(taida_val writer);
taida_val taida_net_sse_event(taida_val writer, taida_val event, taida_val data);
// NB4-6: v4 request body streaming + WebSocket API forward declarations
taida_val taida_net_read_body_chunk(taida_val req);
taida_val taida_net_read_body_all(taida_val req);
taida_val taida_net_ws_upgrade(taida_val req, taida_val writer);
taida_val taida_net_ws_send(taida_val ws, taida_val data);
taida_val taida_net_ws_receive(taida_val ws);
taida_val taida_net_ws_close(taida_val ws, taida_val code);
taida_val taida_net_ws_close_code(taida_val ws);
// v4: body stream request check (defined later, forward declared here for readBody delegation)
static int taida_net4_is_body_stream_request(taida_val req);

// Net result helpers (use HttpError instead of IoError)
static taida_val taida_net_result_ok(taida_val inner) {
    return taida_result_create(inner, 0, 0);
}

static taida_val taida_net_result_fail(const char *kind, const char *message) {
    // inner = @(ok: false, code: -1, message: msg, kind: kind)
    taida_val inner = taida_pack_new(4);
    taida_pack_set_hash(inner, 0, taida_str_hash((taida_val)"ok"));
    taida_pack_set(inner, 0, 0);  // false
    taida_pack_set_tag(inner, 0, TAIDA_TAG_BOOL);
    taida_pack_set_hash(inner, 1, taida_str_hash((taida_val)"code"));
    taida_pack_set(inner, 1, -1);
    taida_pack_set_hash(inner, 2, taida_str_hash((taida_val)"message"));
    taida_pack_set(inner, 2, (taida_val)taida_str_new_copy(message));
    taida_pack_set_tag(inner, 2, TAIDA_TAG_STR);
    taida_pack_set_hash(inner, 3, taida_str_hash((taida_val)"kind"));
    taida_pack_set(inner, 3, (taida_val)taida_str_new_copy(kind));
    taida_pack_set_tag(inner, 3, TAIDA_TAG_STR);

    taida_val error = taida_make_error("HttpError", message);
    return taida_result_create(inner, error, 0);
}

// Helper: create span @(start: Int, len: Int)
static taida_val taida_net_make_span(taida_val start, taida_val len) {
    taida_val pack = taida_pack_new(2);
    taida_pack_set_hash(pack, 0, taida_str_hash((taida_val)"start"));
    taida_pack_set(pack, 0, start);
    taida_pack_set_hash(pack, 1, taida_str_hash((taida_val)"len"));
    taida_pack_set(pack, 1, len);
    return pack;
}

// Status reason phrases (mirrors Interpreter status_reason)
static const char *taida_net_status_reason(int code) {
    switch (code) {
        case 100: return "Continue";
        case 101: return "Switching Protocols";
        case 200: return "OK";
        case 201: return "Created";
        case 202: return "Accepted";
        case 204: return "No Content";
        case 205: return "Reset Content";
        case 206: return "Partial Content";
        case 301: return "Moved Permanently";
        case 302: return "Found";
        case 304: return "Not Modified";
        case 307: return "Temporary Redirect";
        case 308: return "Permanent Redirect";
        case 400: return "Bad Request";
        case 401: return "Unauthorized";
        case 403: return "Forbidden";
        case 404: return "Not Found";
        case 405: return "Method Not Allowed";
        case 408: return "Request Timeout";
        case 409: return "Conflict";
        case 410: return "Gone";
        case 413: return "Content Too Large";
        case 415: return "Unsupported Media Type";
        case 418: return "I'm a Teapot";
        case 422: return "Unprocessable Content";
        case 429: return "Too Many Requests";
        case 500: return "Internal Server Error";
        case 502: return "Bad Gateway";
        case 503: return "Service Unavailable";
        case 504: return "Gateway Timeout";
        default:  return "";
    }
}

// ── httpParseRequestHead(bytes) ─────────────────────────────────
// Hand-written HTTP/1.1 request head parser (no external deps).
// Returns Result[@(complete, consumed, method, path, query, version, headers, bodyOffset, contentLength, chunked), _]
taida_val taida_net_http_parse_request_head(taida_val input) {
    // Extract raw bytes from Bytes or Str
    unsigned char *data = NULL;
    size_t data_len = 0;
    int free_data = 0;

    if (TAIDA_IS_BYTES(input)) {
        taida_val *bytes = (taida_val*)input;
        taida_val blen = bytes[1];
        if (blen < 0) blen = 0;
        data_len = (size_t)blen;
        data = (unsigned char*)TAIDA_MALLOC(data_len + 1, "net_parse_input");
        for (size_t i = 0; i < data_len; i++) data[i] = (unsigned char)bytes[2 + i];
        data[data_len] = 0;
        free_data = 1;
    } else {
        // Assume string
        size_t slen = 0;
        if (!taida_read_cstr_len_safe((const char*)input, 1048576, &slen)) {
            return taida_net_result_fail("ParseError", "httpParseRequestHead: argument must be Bytes or Str");
        }
        data = (unsigned char*)input;
        data_len = slen;
    }

    // Find \r\n\r\n (end of head)
    int head_end = -1;
    for (size_t i = 0; i + 3 < data_len; i++) {
        if (data[i] == '\r' && data[i+1] == '\n' && data[i+2] == '\r' && data[i+3] == '\n') {
            head_end = (int)i;
            break;
        }
    }

    int complete = (head_end >= 0);
    size_t consumed = complete ? (size_t)(head_end + 4) : 0;

    // We need at least a request line to parse
    // Find the first \r\n for request line
    int first_crlf = -1;
    size_t scan_limit = complete ? (size_t)head_end : data_len;
    for (size_t i = 0; i + 1 < scan_limit; i++) {
        if (data[i] == '\r' && data[i+1] == '\n') {
            first_crlf = (int)i;
            break;
        }
    }

    if (first_crlf < 0) {
        // No CRLF found at all — incomplete if no head_end, try to check for obvious malformed
        if (!complete) {
            // Could be incomplete — return incomplete result
            taida_val parsed = taida_pack_new(10);
            taida_pack_set_hash(parsed, 0, taida_str_hash((taida_val)"complete"));
            taida_pack_set(parsed, 0, 0);  // false
            taida_pack_set_tag(parsed, 0, TAIDA_TAG_BOOL);
            taida_pack_set_hash(parsed, 1, taida_str_hash((taida_val)"consumed"));
            taida_pack_set(parsed, 1, 0);
            taida_pack_set_hash(parsed, 2, taida_str_hash((taida_val)"method"));
            taida_pack_set(parsed, 2, taida_net_make_span(0, 0));
            taida_pack_set_tag(parsed, 2, TAIDA_TAG_PACK);
            taida_pack_set_hash(parsed, 3, taida_str_hash((taida_val)"path"));
            taida_pack_set(parsed, 3, taida_net_make_span(0, 0));
            taida_pack_set_tag(parsed, 3, TAIDA_TAG_PACK);
            taida_pack_set_hash(parsed, 4, taida_str_hash((taida_val)"query"));
            taida_pack_set(parsed, 4, taida_net_make_span(0, 0));
            taida_pack_set_tag(parsed, 4, TAIDA_TAG_PACK);
            taida_val ver = taida_pack_new(2);
            taida_pack_set_hash(ver, 0, taida_str_hash((taida_val)"major"));
            taida_pack_set(ver, 0, 1);
            taida_pack_set_hash(ver, 1, taida_str_hash((taida_val)"minor"));
            taida_pack_set(ver, 1, 1);
            taida_pack_set_hash(parsed, 5, taida_str_hash((taida_val)"version"));
            taida_pack_set(parsed, 5, ver);
            taida_pack_set_tag(parsed, 5, TAIDA_TAG_PACK);
            taida_pack_set_hash(parsed, 6, taida_str_hash((taida_val)"headers"));
            taida_pack_set(parsed, 6, taida_list_new());
            taida_pack_set_tag(parsed, 6, TAIDA_TAG_LIST);
            taida_pack_set_hash(parsed, 7, taida_str_hash((taida_val)"bodyOffset"));
            taida_pack_set(parsed, 7, 0);
            taida_pack_set_hash(parsed, 8, taida_str_hash((taida_val)"contentLength"));
            taida_pack_set(parsed, 8, 0);
            taida_pack_set_hash(parsed, 9, taida_str_hash((taida_val)"chunked"));
            taida_pack_set(parsed, 9, 0);  // false
            taida_pack_set_tag(parsed, 9, TAIDA_TAG_BOOL);
            if (free_data) free(data);
            return taida_net_result_ok(parsed);
        }
        if (free_data) free(data);
        return taida_net_result_fail("ParseError", "Malformed HTTP request: no request line");
    }

    // Parse request line: METHOD SP PATH HTTP/x.y
    // Find first SP
    int method_end = -1;
    for (int i = 0; i < first_crlf; i++) {
        if (data[i] == ' ') { method_end = i; break; }
    }
    if (method_end <= 0) {
        if (free_data) free(data);
        return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid request line");
    }

    // Find last SP (before HTTP/x.y)
    int version_start = -1;
    for (int i = first_crlf - 1; i > method_end; i--) {
        if (data[i - 1] == ' ') { version_start = i; break; }
    }
    if (version_start < 0 || version_start <= method_end + 1) {
        if (free_data) free(data);
        return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid request line");
    }

    // Parse version: must be exactly "HTTP/x.y" where x,y are single ASCII digits
    // Strict: reject HTTP/a.b, HTTP/12.34, HTTP/1, etc. (parity with Interpreter/JS)
    int http_major = 1, http_minor = 1;
    int version_len = first_crlf - version_start;
    if (version_len == 8 &&
        data[version_start]   == 'H' && data[version_start+1] == 'T' &&
        data[version_start+2] == 'T' && data[version_start+3] == 'P' &&
        data[version_start+4] == '/' &&
        data[version_start+5] >= '0' && data[version_start+5] <= '9' &&
        data[version_start+6] == '.' &&
        data[version_start+7] >= '0' && data[version_start+7] <= '9') {
        http_major = data[version_start+5] - '0';
        http_minor = data[version_start+7] - '0';
        // NB-32: restrict to HTTP/1.0 and HTTP/1.1 only (parity with Interpreter/httparse)
        // Reject immediately once version is fully parsed, regardless of head completeness
        if (http_major != 1 || (http_minor != 0 && http_minor != 1)) {
            if (free_data) free(data);
            return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid HTTP version");
        }
    } else if (complete) {
        if (free_data) free(data);
        return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid HTTP version");
    }

    // Method span
    int method_start_idx = 0;
    int method_len = method_end;

    // C26B-022 Step 2 (wS Round 6, 2026-04-24): HTTP wire byte upper
    // limits must be enforced at the Native parser boundary so that the
    // fixed-size struct fields downstream (`char method[16]` in
    // net_h1_h2.c H2RequestFields and net_h3_quic.c H3RequestFields)
    // cannot silently truncate. Must match the interpreter constants in
    // src/interpreter/net_eval/h1.rs::HTTP_WIRE_MAX_METHOD_LEN (=16).
    // Option confirmation: Step 3 Option B (parser reject with 400).
    if (method_len > 16) {
        if (free_data) free(data);
        return taida_net_result_fail("ParseError", "Malformed HTTP request: method exceeds wire-byte limit");
    }

    // Path + query: between first SP and last SP
    int uri_start = method_end + 1;
    int uri_end = version_start - 1;
    int uri_len = uri_end - uri_start;

    // Split path and query on '?'
    int path_start_idx = uri_start;
    int path_len = uri_len;
    int query_start_idx = 0;
    int query_len = 0;
    for (int i = uri_start; i < uri_end; i++) {
        if (data[i] == '?') {
            path_len = i - uri_start;
            query_start_idx = i + 1;
            query_len = uri_end - (i + 1);
            break;
        }
    }

    // C26B-022 Step 2 (wS Round 6): path wire-byte cap = 2048 (matches
    // HTTP_WIRE_MAX_PATH_LEN in h1.rs and `char path[2048]` field).
    if (path_len > 2048) {
        if (free_data) free(data);
        return taida_net_result_fail("ParseError", "Malformed HTTP request: path exceeds wire-byte limit");
    }

    // Parse headers
    taida_val headers_list = taida_list_new();
    int64_t content_length = 0;
    int cl_count = 0;
    int has_te_chunked = 0;
    size_t pos = (size_t)(first_crlf + 2);  // after first \r\n

    int header_count = 0;
    while (pos < scan_limit) {
        // Find next \r\n
        size_t line_end = scan_limit;
        for (size_t j = pos; j + 1 < scan_limit; j++) {
            if (data[j] == '\r' && data[j+1] == '\n') {
                line_end = j;
                break;
            }
        }
        if (line_end == pos) break;  // empty line = end of headers

        // NB-4/NB-6: enforce max 64 headers (parity with Interpreter/httparse)
        header_count++;
        if (header_count > 64) {
            if (free_data) free(data);
            return taida_net_result_fail("ParseError", "Malformed HTTP request: too many headers");
        }

        // Find colon separator
        size_t colon = line_end;
        for (size_t j = pos; j < line_end; j++) {
            if (data[j] == ':') { colon = j; break; }
        }
        if (colon >= line_end) {
            // No colon found: if head is complete this is malformed, otherwise incomplete
            if (complete) {
                if (free_data) free(data);
                return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid header line");
            }
            break;  // incomplete — stop parsing headers
        }

        // Header name: pos..colon, value: after colon + OWS trimming
        size_t name_start = pos;
        size_t name_len = colon - pos;
        size_t val_start = colon + 1;
        // NB-34: Skip leading SP/HT and trim trailing SP/HT (parity with Interpreter/httparse)
        while (val_start < line_end && (data[val_start] == ' ' || data[val_start] == '\t')) val_start++;
        size_t val_end = line_end;
        while (val_end > val_start && (data[val_end - 1] == ' ' || data[val_end - 1] == '\t')) val_end--;
        size_t val_len = val_end - val_start;

        taida_val header_pack = taida_pack_new(2);
        taida_pack_set_hash(header_pack, 0, taida_str_hash((taida_val)"name"));
        taida_pack_set(header_pack, 0, taida_net_make_span((taida_val)name_start, (taida_val)name_len));
        taida_pack_set_tag(header_pack, 0, TAIDA_TAG_PACK);
        taida_pack_set_hash(header_pack, 1, taida_str_hash((taida_val)"value"));
        taida_pack_set(header_pack, 1, taida_net_make_span((taida_val)val_start, (taida_val)val_len));
        taida_pack_set_tag(header_pack, 1, TAIDA_TAG_PACK);
        headers_list = taida_list_push(headers_list, header_pack);

        // C26B-022 Step 2 (wS Round 6): authority wire-byte cap = 256.
        // Host is the HTTP/1.x equivalent of the H2/H3 `:authority`
        // pseudo-header; the `char authority[256]` struct field in
        // H2RequestFields / H3RequestFields backs both paths, so reject
        // over-limit Host values at parse time to match h1.rs.
        if (name_len == 4) {
            const char *host_expected = "host";
            int is_host = 1;
            for (size_t k = 0; k < 4; k++) {
                char c = (char)data[name_start + k];
                if (c >= 'A' && c <= 'Z') c += 32;
                if (c != host_expected[k]) { is_host = 0; break; }
            }
            if (is_host && val_len > 256) {
                if (free_data) free(data);
                return taida_net_result_fail("ParseError", "Malformed HTTP request: authority exceeds wire-byte limit");
            }
        }

        // Check Content-Length (case-insensitive)
        if (name_len == 14) {
            // Check "content-length" case-insensitively
            const char *cl_expected = "content-length";
            int is_cl = 1;
            for (size_t k = 0; k < 14; k++) {
                char c = (char)data[name_start + k];
                if (c >= 'A' && c <= 'Z') c += 32;
                if (c != cl_expected[k]) { is_cl = 0; break; }
            }
            if (is_cl) {
                cl_count++;
                if (cl_count > 1) {
                    if (free_data) free(data);
                    return taida_net_result_fail("ParseError", "Malformed HTTP request: duplicate Content-Length header");
                }
                // Validate: trimmed value must be all digits
                // val_start..val_start+val_len (already trimmed leading spaces/tabs)
                // Also trim trailing spaces and tabs (parity with Interpreter's .trim() and JS's .trim())
                size_t cl_end = val_start + val_len;
                while (cl_end > val_start && (data[cl_end-1] == ' ' || data[cl_end-1] == '\t')) cl_end--;
                size_t cl_len = cl_end - val_start;
                if (cl_len == 0) {
                    if (free_data) free(data);
                    return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid Content-Length value");
                }
                int all_digits = 1;
                for (size_t k = 0; k < cl_len; k++) {
                    if (data[val_start + k] < '0' || data[val_start + k] > '9') {
                        all_digits = 0;
                        break;
                    }
                }
                if (!all_digits) {
                    if (free_data) free(data);
                    return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid Content-Length value");
                }
                // Parse digits
                int64_t cl_val = 0;
                for (size_t k = 0; k < cl_len; k++) {
                    int64_t digit = data[val_start + k] - '0';
                    // Overflow check
                    if (cl_val > (9007199254740991LL - digit) / 10) {
                        if (free_data) free(data);
                        return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid Content-Length value");
                    }
                    cl_val = cl_val * 10 + digit;
                }
                // Cap at Number.MAX_SAFE_INTEGER
                if (cl_val > 9007199254740991LL) {
                    if (free_data) free(data);
                    return taida_net_result_fail("ParseError", "Malformed HTTP request: invalid Content-Length value");
                }
                content_length = cl_val;
            }
        }

        // NET2-2a: Detect Transfer-Encoding: chunked (parity with Interpreter)
        if (name_len == 17) {
            const char *te_expected = "transfer-encoding";
            int is_te = 1;
            for (size_t k = 0; k < 17; k++) {
                char c = (char)data[name_start + k];
                if (c >= 'A' && c <= 'Z') c += 32;
                if (c != te_expected[k]) { is_te = 0; break; }
            }
            if (is_te) {
                // Scan comma-separated tokens for "chunked" (case-insensitive)
                size_t tk_start = val_start;
                while (tk_start < val_start + val_len) {
                    // Skip leading whitespace
                    while (tk_start < val_start + val_len && (data[tk_start] == ' ' || data[tk_start] == '\t')) tk_start++;
                    // Find comma or end
                    size_t tk_end = tk_start;
                    while (tk_end < val_start + val_len && data[tk_end] != ',') tk_end++;
                    // Trim trailing whitespace of token
                    size_t tk_trim = tk_end;
                    while (tk_trim > tk_start && (data[tk_trim - 1] == ' ' || data[tk_trim - 1] == '\t')) tk_trim--;
                    size_t tk_len = tk_trim - tk_start;
                    if (tk_len == 7) {
                        const char *chunked_str = "chunked";
                        int match = 1;
                        for (size_t m = 0; m < 7; m++) {
                            char c = (char)data[tk_start + m];
                            if (c >= 'A' && c <= 'Z') c += 32;
                            if (c != chunked_str[m]) { match = 0; break; }
                        }
                        if (match) has_te_chunked = 1;
                    }
                    tk_start = tk_end + 1;  // skip comma
                }
            }
        }

        pos = line_end + 2;  // skip \r\n
    }

    // NET2-2e: Reject Content-Length + Transfer-Encoding: chunked (RFC 7230 section 3.3.3)
    if (has_te_chunked && cl_count > 0) {
        if (free_data) free(data);
        return taida_net_result_fail("ParseError", "Malformed HTTP request: Content-Length and Transfer-Encoding: chunked are mutually exclusive");
    }

    // Build result pack
    taida_val parsed = taida_pack_new(10);
    taida_pack_set_hash(parsed, 0, taida_str_hash((taida_val)"complete"));
    taida_pack_set(parsed, 0, complete ? 1 : 0);
    taida_pack_set_tag(parsed, 0, TAIDA_TAG_BOOL);
    taida_pack_set_hash(parsed, 1, taida_str_hash((taida_val)"consumed"));
    taida_pack_set(parsed, 1, (taida_val)consumed);
    taida_pack_set_hash(parsed, 2, taida_str_hash((taida_val)"method"));
    taida_pack_set(parsed, 2, taida_net_make_span((taida_val)method_start_idx, (taida_val)method_len));
    taida_pack_set_tag(parsed, 2, TAIDA_TAG_PACK);
    taida_pack_set_hash(parsed, 3, taida_str_hash((taida_val)"path"));
    taida_pack_set(parsed, 3, taida_net_make_span((taida_val)path_start_idx, (taida_val)path_len));
    taida_pack_set_tag(parsed, 3, TAIDA_TAG_PACK);
    taida_pack_set_hash(parsed, 4, taida_str_hash((taida_val)"query"));
    taida_pack_set(parsed, 4, taida_net_make_span((taida_val)query_start_idx, (taida_val)query_len));
    taida_pack_set_tag(parsed, 4, TAIDA_TAG_PACK);

    taida_val ver = taida_pack_new(2);
    taida_pack_set_hash(ver, 0, taida_str_hash((taida_val)"major"));
    taida_pack_set(ver, 0, (taida_val)http_major);
    taida_pack_set_hash(ver, 1, taida_str_hash((taida_val)"minor"));
    taida_pack_set(ver, 1, (taida_val)http_minor);
    taida_pack_set_hash(parsed, 5, taida_str_hash((taida_val)"version"));
    taida_pack_set(parsed, 5, ver);
    taida_pack_set_tag(parsed, 5, TAIDA_TAG_PACK);

    taida_pack_set_hash(parsed, 6, taida_str_hash((taida_val)"headers"));
    taida_pack_set(parsed, 6, headers_list);
    taida_pack_set_tag(parsed, 6, TAIDA_TAG_LIST);

    taida_pack_set_hash(parsed, 7, taida_str_hash((taida_val)"bodyOffset"));
    taida_pack_set(parsed, 7, (taida_val)consumed);

    taida_pack_set_hash(parsed, 8, taida_str_hash((taida_val)"contentLength"));
    taida_pack_set(parsed, 8, (taida_val)content_length);

    taida_pack_set_hash(parsed, 9, taida_str_hash((taida_val)"chunked"));
    taida_pack_set(parsed, 9, has_te_chunked ? 1 : 0);
    taida_pack_set_tag(parsed, 9, TAIDA_TAG_BOOL);

    if (free_data) free(data);
    return taida_net_result_ok(parsed);
}

// ── httpEncodeResponse(response) ────────────────────────────────
// Encode response @(status, headers, body) into HTTP/1.1 wire bytes.
// Returns Result[@(bytes: Bytes), _]
taida_val taida_net_http_encode_response(taida_val response) {
    if (!taida_is_buchi_pack(response)) {
        return taida_net_result_fail("EncodeError", "httpEncodeResponse: argument must be a BuchiPack @(...)");
    }

    // Extract status (required, must be Int in 100-999)
    taida_val status_hash = taida_str_hash((taida_val)"status");
    if (!taida_pack_has_hash(response, status_hash)) {
        return taida_net_result_fail("EncodeError", "httpEncodeResponse: missing required field 'status'");
    }
    taida_val status = taida_pack_get(response, status_hash);
    // NB-14: Type check via field tag — status must be Int.
    // When tag is UNKNOWN, resolve via runtime detection to catch non-Int values
    // that the compiler couldn't type-check statically.
    {
        taida_val status_tag = taida_pack_get_field_tag(response, status_hash);
        if (status_tag == TAIDA_TAG_UNKNOWN) {
            status_tag = taida_runtime_detect_tag(status);
        }
        if (status_tag != TAIDA_TAG_INT) {
            char val_buf[64];
            taida_format_value(status_tag, status, val_buf, sizeof(val_buf));
            char err_msg[128];
            snprintf(err_msg, sizeof(err_msg),
                     "httpEncodeResponse: status must be Int, got %s",
                     val_buf);
            return taida_net_result_fail("EncodeError", err_msg);
        }
    }
    if (status < 100 || status > 999) {
        char err_msg[128];
        snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: status must be 100-999, got %d", (int)status);
        return taida_net_result_fail("EncodeError", err_msg);
    }

    // Extract headers (required, must be a List)
    taida_val headers_hash = taida_str_hash((taida_val)"headers");
    if (!taida_pack_has_hash(response, headers_hash)) {
        return taida_net_result_fail("EncodeError", "httpEncodeResponse: missing required field 'headers'");
    }
    taida_val headers_ptr = taida_pack_get(response, headers_hash);
    if (!taida_is_list(headers_ptr)) {
        // NB-21: Format actual value for parity with Interpreter/JS
        taida_val htag = taida_pack_get_field_tag(response, headers_hash);
        char val_buf[64];
        taida_format_value(htag, headers_ptr, val_buf, sizeof(val_buf));
        char err_msg[128];
        snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: headers must be a List, got %s",
                 val_buf);
        return taida_net_result_fail("EncodeError", err_msg);
    }

    // Extract body (required, must be Bytes or Str)
    // NB6-4: For Bytes, defer materialization until the wire buffer is ready.
    // Instead of allocating a separate body_data buffer and copying twice
    // (taida_val -> body_data -> wire buf), we record the source pointer and
    // copy directly into the wire buffer once.
    taida_val body_hash = taida_str_hash((taida_val)"body");
    if (!taida_pack_has_hash(response, body_hash)) {
        return taida_net_result_fail("EncodeError", "httpEncodeResponse: missing required field 'body'");
    }
    taida_val body_ptr = taida_pack_get(response, body_hash);
    unsigned char *body_data = NULL;  // contiguous body (Str path or D29 contig Bytes path)
    taida_val *body_bytes_arr = NULL; // taida_val array (legacy Bytes path only)
    size_t body_len = 0;
    int body_is_bytes = 0;
    // D29B-003 (Track-β): contig-bytes fast path. When set, body_data
    // already points at the inline contiguous payload of TAIDA_BYTES_CONTIG
    // and the byte-loop materialize below is skipped.
    int body_is_contig = 0;

    if (TAIDA_IS_BYTES_CONTIG(body_ptr)) {
        body_data = (unsigned char *)taida_bytes_contig_data(body_ptr);
        taida_val blen = taida_bytes_contig_len(body_ptr);
        if (blen < 0) blen = 0;
        body_len = (size_t)blen;
        body_is_bytes = 1;
        body_is_contig = 1;
    } else if (TAIDA_IS_BYTES(body_ptr)) {
        body_bytes_arr = (taida_val*)body_ptr;
        taida_val blen = body_bytes_arr[1];
        if (blen < 0) blen = 0;
        body_len = (size_t)blen;
        body_is_bytes = 1;
    } else {
        size_t slen = 0;
        if (taida_read_cstr_len_safe((const char*)body_ptr, 10485760, &slen)) {
            body_data = (unsigned char*)body_ptr;
            body_len = slen;
        } else {
            // NB-21: Format actual value for parity with Interpreter/JS
            taida_val btag = taida_pack_get_field_tag(response, body_hash);
            char val_buf[64];
            taida_format_value(btag, body_ptr, val_buf, sizeof(val_buf));
            char err_msg[128];
            snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: body must be Bytes or Str, got %s",
                     val_buf);
            return taida_net_result_fail("EncodeError", err_msg);
        }
    }

    // RFC 9110: 1xx, 204, 205, 304 MUST NOT contain a message body
    int no_body = (status >= 100 && status < 200) || status == 204 || status == 205 || status == 304;
    if (no_body && body_len > 0) {
        char err_msg[128];
        snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: status %d must not have a body", (int)status);
        return taida_net_result_fail("EncodeError", err_msg);
    }

    // Build HTTP response buffer
    size_t buf_cap = 512 + body_len;
    unsigned char *buf = (unsigned char*)TAIDA_MALLOC(buf_cap, "net_encode_buf");
    size_t buf_len = 0;

    // Status line
    const char *reason = taida_net_status_reason((int)status);
    buf_len += (size_t)snprintf((char*)buf + buf_len, buf_cap - buf_len,
                                 "HTTP/1.1 %d %s\r\n", (int)status, reason);

    // User headers
    int has_content_length = 0;
    taida_val name_hash = taida_str_hash((taida_val)"name");
    taida_val value_hash = taida_str_hash((taida_val)"value");

    {
        taida_val *hlist = (taida_val*)headers_ptr;
        taida_val hcount = hlist[2];
        for (taida_val i = 0; i < hcount; i++) {
            taida_val hdr = hlist[4 + i];
            if (!taida_is_buchi_pack(hdr)) {
                free(buf);
                char err_msg[128];
                snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: headers[%d] must be @(name, value)", (int)i);
                return taida_net_result_fail("EncodeError", err_msg);
            }
            taida_val hname = taida_pack_get(hdr, name_hash);
            taida_val hvalue = taida_pack_get(hdr, value_hash);
            const char *hname_s = (const char*)hname;
            const char *hvalue_s = (const char*)hvalue;
            size_t hn_len = 0, hv_len = 0;
            if (!taida_read_cstr_len_safe(hname_s, 8192, &hn_len)) {
                free(buf);
                char err_msg[128];
                snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: headers[%d].name must be Str", (int)i);
                return taida_net_result_fail("EncodeError", err_msg);
            }
            if (!taida_read_cstr_len_safe(hvalue_s, 65536, &hv_len)) {
                free(buf);
                char err_msg[128];
                snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: headers[%d].value must be Str", (int)i);
                return taida_net_result_fail("EncodeError", err_msg);
            }

            // NB-13: Check for CRLF injection with index + name/value distinction (parity with Interpreter/JS)
            for (size_t k = 0; k < hn_len; k++) {
                if (hname_s[k] == '\r' || hname_s[k] == '\n') {
                    free(buf);
                    char err_msg[128];
                    snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: headers[%d].name contains CR/LF", (int)i);
                    return taida_net_result_fail("EncodeError", err_msg);
                }
            }
            for (size_t k = 0; k < hv_len; k++) {
                if (hvalue_s[k] == '\r' || hvalue_s[k] == '\n') {
                    free(buf);
                    char err_msg[128];
                    snprintf(err_msg, sizeof(err_msg), "httpEncodeResponse: headers[%d].value contains CR/LF", (int)i);
                    return taida_net_result_fail("EncodeError", err_msg);
                }
            }

            // Skip Content-Length for no-body statuses
            if (no_body && hn_len == 14) {
                const char *cl_expected = "content-length";
                int is_cl = 1;
                for (size_t k = 0; k < 14; k++) {
                    char c = hname_s[k];
                    if (c >= 'A' && c <= 'Z') c += 32;
                    if (c != cl_expected[k]) { is_cl = 0; break; }
                }
                if (is_cl) continue;
            }

            // Check if user provided Content-Length
            if (hn_len == 14) {
                const char *cl_expected = "content-length";
                int is_cl = 1;
                for (size_t k = 0; k < 14; k++) {
                    char c = hname_s[k];
                    if (c >= 'A' && c <= 'Z') c += 32;
                    if (c != cl_expected[k]) { is_cl = 0; break; }
                }
                if (is_cl) has_content_length = 1;
            }

            // Grow buffer if needed
            size_t needed = buf_len + hn_len + hv_len + 4;
            if (needed > buf_cap) {
                buf_cap = needed * 2;
                TAIDA_REALLOC(buf, buf_cap, "net_encode_headers");
            }
            memcpy(buf + buf_len, hname_s, hn_len); buf_len += hn_len;
            buf[buf_len++] = ':'; buf[buf_len++] = ' ';
            memcpy(buf + buf_len, hvalue_s, hv_len); buf_len += hv_len;
            buf[buf_len++] = '\r'; buf[buf_len++] = '\n';
        }
    }

    // Auto-append Content-Length for statuses that allow a body
    if (!no_body && !has_content_length) {
        char cl_hdr[64];
        int cl_len = snprintf(cl_hdr, sizeof(cl_hdr), "Content-Length: %zu\r\n", body_len);
        size_t needed = buf_len + (size_t)cl_len;
        if (needed > buf_cap) {
            buf_cap = needed * 2;
            TAIDA_REALLOC(buf, buf_cap, "net_encode_cl");
        }
        memcpy(buf + buf_len, cl_hdr, (size_t)cl_len);
        buf_len += (size_t)cl_len;
    }

    // End of headers
    size_t needed = buf_len + 2 + body_len;
    if (needed > buf_cap) {
        buf_cap = needed;
        TAIDA_REALLOC(buf, buf_cap, "net_encode_body");
    }
    buf[buf_len++] = '\r'; buf[buf_len++] = '\n';

    // NB6-4: Copy body directly into wire buffer — single copy from source.
    // For Bytes (legacy taida_val[]): copy via byte-loop from taida_val array.
    // For Bytes (D29B-003 contig): memcpy from body_data which already points
    //   at the inline contiguous payload — same fast path as the Str case.
    // For Str: memcpy from C string pointer (already contiguous).
    if (!no_body && body_len > 0) {
        if (body_is_bytes && !body_is_contig) {
            for (size_t i = 0; i < body_len; i++) {
                buf[buf_len + i] = (unsigned char)body_bytes_arr[2 + i];
            }
        } else {
            // Str OR D29B-003 contig Bytes: contiguous memcpy.
            memcpy(buf + buf_len, body_data, body_len);
        }
        buf_len += body_len;
    }

    // D29B-003 (Track-β, 2026-04-27): kept on legacy taida_bytes_from_raw
    // for now. The CONTIG hot path is opt-in via addon-side construction
    // (see taida_bytes_contig_new / TAIDA_BYTES_CONTIG_MAGIC) so producers
    // that round-trip Bytes through length / get / to_list / decode molds
    // (which still index `taida_val[]` slots directly) continue to work
    // unchanged. A follow-up sub-Lock will polymorphize the remaining Bytes
    // dispatchers and switch this producer to taida_bytes_contig_new for
    // the full payload-level zero-copy chain.
    taida_val result_bytes = taida_bytes_from_raw(buf, (taida_val)buf_len);
    free(buf);

    taida_val result = taida_pack_new(1);
    taida_pack_set_hash(result, 0, taida_str_hash((taida_val)"bytes"));
    taida_pack_set(result, 0, result_bytes);
    taida_pack_set_tag(result, 0, TAIDA_TAG_PACK);  // Bytes IS-A tagged ptr

    return taida_net_result_ok(result);
}

// ── net_send_all: short-write safe send helper ──────────────────
// Loops send() until all bytes are written or an error occurs.
// Returns 0 on success, -1 on error.
// NET5-4a: Routes through TLS when tl_ssl is active.
static int taida_net_send_all(int fd, const void *buf, size_t len) {
    return taida_tls_send_all(fd, buf, len);
}

// ── readBody(req) → Bytes ────────────────────────────────────────
// Extract body bytes from a request pack.
// req.raw (Bytes) + body span (start, len) → body slice as new Bytes.
// If body.len == 0 or body span is absent, returns empty Bytes.
taida_val taida_net_read_body(taida_val req) {
    if (!taida_is_buchi_pack(req)) {
        // Parity: Interpreter returns RuntimeError, JS throws __NativeError
        char val_buf[64];
        taida_val tag = taida_runtime_detect_tag(req);
        taida_format_value(tag, req, val_buf, sizeof(val_buf));
        char err_msg[256];
        snprintf(err_msg, sizeof(err_msg),
                 "readBody: argument must be a request pack @(...), got %s",
                 val_buf);
        return taida_throw(taida_make_error("TypeError", err_msg));
    }

    // v4: If the request has __body_stream sentinel (2-arg handler),
    // delegate to readBodyAll to stream from socket.
    if (taida_net4_is_body_stream_request(req)) {
        return taida_net_read_body_all(req);
    }

    // Extract raw: Bytes
    taida_val raw = taida_pack_get(req, taida_str_hash((taida_val)"raw"));
    if (!TAIDA_IS_BYTES(raw)) {
        return taida_throw(taida_make_error("TypeError",
            "readBody: request pack missing 'raw: Bytes' field"));
    }

    // Extract body: @(start: Int, len: Int)
    taida_val body_span = taida_pack_get(req, taida_str_hash((taida_val)"body"));
    taida_val body_start = 0;
    taida_val body_len = 0;
    if (body_span != 0 && taida_is_buchi_pack(body_span)) {
        body_start = taida_pack_get(body_span, taida_str_hash((taida_val)"start"));
        body_len = taida_pack_get(body_span, taida_str_hash((taida_val)"len"));
    }

    if (body_len <= 0) {
        return taida_bytes_new_filled(0, 0);
    }

    // raw layout: [magic+rc, length, b0, b1, ...]
    taida_val *raw_arr = (taida_val*)raw;
    taida_val raw_len = raw_arr[1];

    // Clamp to valid range
    if (body_start < 0) body_start = 0;
    if (body_start > raw_len) body_start = raw_len;
    taida_val end = body_start + body_len;
    if (end > raw_len) end = raw_len;
    taida_val actual_len = end - body_start;
    if (actual_len <= 0) {
        return taida_bytes_new_filled(0, 0);
    }

    // Copy body bytes into a new Bytes object
    taida_val out = taida_bytes_new_filled(actual_len, 0);
    taida_val *out_arr = (taida_val*)out;
    for (taida_val i = 0; i < actual_len; i++) {
        out_arr[2 + i] = raw_arr[2 + body_start + i];
    }
    return out;
}

// ── NET2-5a: Keep-Alive determination ──────────────────────────
// Determine whether the connection should be kept alive based on
// HTTP version and the Connection header value.
// Rules (RFC 7230 S6.1):
//   HTTP/1.1: keep-alive by default, Connection: close disables
//   HTTP/1.0: close by default, Connection: keep-alive enables
// raw is the wire bytes buffer, headers is the parsed header list.
static int taida_net_determine_keep_alive(
    const unsigned char *raw, size_t raw_len,
    taida_val headers, taida_val http_minor
) {
    int has_close = 0;
    int has_keep_alive = 0;

    if (!TAIDA_IS_LIST(headers)) {
        return (http_minor == 1) ? 1 : 0;
    }

    taida_val *hdr_list = (taida_val*)headers;
    taida_val hdr_count = hdr_list[2];  // list length at index 2 (layout: [magic+rc, capacity, length, elem_tag, ...])

    for (taida_val i = 0; i < hdr_count; i++) {
        taida_val header = hdr_list[4 + i];
        if (!taida_is_buchi_pack(header)) continue;

        // Get name span
        taida_val name_span = taida_pack_get(header, taida_str_hash((taida_val)"name"));
        if (!taida_is_buchi_pack(name_span)) continue;
        taida_val name_start = taida_pack_get(name_span, taida_str_hash((taida_val)"start"));
        taida_val name_len = taida_pack_get(name_span, taida_str_hash((taida_val)"len"));
        if (name_start < 0 || name_len <= 0) continue;
        if ((size_t)(name_start + name_len) > raw_len) continue;

        // Case-insensitive compare with "connection" (10 chars)
        if (name_len != 10) continue;
        const char *conn_str = "connection";
        int match = 1;
        for (int j = 0; j < 10; j++) {
            char c = (char)raw[name_start + j];
            if (c >= 'A' && c <= 'Z') c += 32;
            if (c != conn_str[j]) { match = 0; break; }
        }
        if (!match) continue;

        // Extract value span and scan comma-separated tokens
        taida_val val_span = taida_pack_get(header, taida_str_hash((taida_val)"value"));
        if (!taida_is_buchi_pack(val_span)) continue;
        taida_val val_start = taida_pack_get(val_span, taida_str_hash((taida_val)"start"));
        taida_val val_len = taida_pack_get(val_span, taida_str_hash((taida_val)"len"));
        if (val_start < 0 || val_len <= 0) continue;
        if ((size_t)(val_start + val_len) > raw_len) continue;

        // Scan tokens split by ','
        const unsigned char *vp = raw + val_start;
        size_t vl = (size_t)val_len;
        size_t tok_start = 0;
        for (size_t k = 0; k <= vl; k++) {
            if (k == vl || vp[k] == ',') {
                // Trim whitespace
                size_t ts = tok_start, te = k;
                while (ts < te && (vp[ts] == ' ' || vp[ts] == '\t')) ts++;
                while (te > ts && (vp[te-1] == ' ' || vp[te-1] == '\t')) te--;
                size_t tlen = te - ts;
                if (tlen == 5) {
                    // "close"
                    int mc = 1;
                    const char *cs = "close";
                    for (size_t m = 0; m < 5; m++) {
                        char c = (char)vp[ts + m];
                        if (c >= 'A' && c <= 'Z') c += 32;
                        if (c != cs[m]) { mc = 0; break; }
                    }
                    if (mc) has_close = 1;
                } else if (tlen == 10) {
                    // "keep-alive"
                    int mk = 1;
                    const char *ks = "keep-alive";
                    for (size_t m = 0; m < 10; m++) {
                        char c = (char)vp[ts + m];
                        if (c >= 'A' && c <= 'Z') c += 32;
                        if (c != ks[m]) { mk = 0; break; }
                    }
                    if (mk) has_keep_alive = 1;
                }
                tok_start = k + 1;
            }
        }
        // Don't break — merge multiple Connection headers
    }

    // RFC 7230 S6.1: close always wins
    if (has_close) return 0;
    if (http_minor == 1) return 1;  // HTTP/1.1 default keep-alive
    return has_keep_alive ? 1 : 0;  // HTTP/1.0 default close
}

// ── NET2-5b: Chunked in-place compaction ────────────────────────
// Result struct for chunked compaction
typedef struct {
    size_t body_len;       // compacted body length
    size_t wire_consumed;  // total bytes consumed from body_offset
} ChunkedCompactResult;

// Find the first CRLF in buf[0..len). Returns offset of '\r', or -1 if not found.
static int64_t taida_net_find_crlf(const unsigned char *data, size_t len) {
    if (len < 2) return -1;
    for (size_t i = 0; i + 1 < len; i++) {
        if (data[i] == '\r' && data[i + 1] == '\n') return (int64_t)i;
    }
    return -1;
}

// Check if a complete chunked body is available (read-only scan).
// Returns wire_consumed on success, -1 if incomplete, -2 if malformed.
static int64_t taida_net_chunked_body_complete(
    const unsigned char *buf, size_t total_len, size_t body_offset
) {
    size_t data_len = total_len - body_offset;
    size_t rp = 0;

    for (;;) {
        if (rp >= data_len) return -1; // incomplete

        int64_t crlf = taida_net_find_crlf(buf + body_offset + rp, data_len - rp);
        if (crlf < 0) return -1; // incomplete

        // Parse hex chunk-size, ignoring chunk-ext after ';'
        size_t hex_end = (size_t)crlf;
        for (size_t i = 0; i < hex_end; i++) {
            if (buf[body_offset + rp + i] == ';') { hex_end = i; break; }
        }
        // Trim whitespace
        size_t hs = 0, he = hex_end;
        while (hs < he && (buf[body_offset + rp + hs] == ' ' || buf[body_offset + rp + hs] == '\t')) hs++;
        while (he > hs && (buf[body_offset + rp + he - 1] == ' ' || buf[body_offset + rp + he - 1] == '\t')) he--;
        if (hs >= he) return -2; // empty chunk-size = malformed

        // Parse hex
        // NB2-5: Reject chunk-size with more than 15 hex digits (max safe: 0xFFFFFFFFFFFFFFF)
        // to prevent size_t overflow that silently wraps to 0 and accepts malformed input.
        if (he - hs > 15) return -2; // oversized chunk-size = malformed
        size_t chunk_size = 0;
        for (size_t i = hs; i < he; i++) {
            unsigned char c = buf[body_offset + rp + i];
            int digit = -1;
            if (c >= '0' && c <= '9') digit = c - '0';
            else if (c >= 'a' && c <= 'f') digit = 10 + c - 'a';
            else if (c >= 'A' && c <= 'F') digit = 10 + c - 'A';
            if (digit < 0) return -2; // invalid hex
            chunk_size = chunk_size * 16 + (size_t)digit;
        }

        rp += (size_t)crlf + 2; // skip "chunk-size\r\n"

        if (chunk_size == 0) {
            // Terminator chunk: skip trailers
            for (;;) {
                if (rp + 2 > data_len) return -1; // incomplete
                if (buf[body_offset + rp] == '\r' && buf[body_offset + rp + 1] == '\n') {
                    rp += 2;
                    return (int64_t)rp;
                }
                int64_t tc = taida_net_find_crlf(buf + body_offset + rp, data_len - rp);
                if (tc < 0) return -1; // incomplete
                rp += (size_t)tc + 2;
            }
        }

        // Check data + CRLF
        if (rp + chunk_size + 2 > data_len) return -1; // incomplete
        rp += chunk_size;
        if (buf[body_offset + rp] != '\r' || buf[body_offset + rp + 1] != '\n') return -2; // malformed
        rp += 2;
    }
}

// In-place compaction: remove chunk framing, compact data in-place using memmove.
// Returns 0 on success (result written to *out), -1 on error.
static int taida_net_chunked_in_place_compact(
    unsigned char *buf, size_t body_offset, ChunkedCompactResult *out
) {
    size_t rp = 0; // read position relative to body_offset
    size_t wp = 0; // write position relative to body_offset

    for (;;) {
        int64_t crlf = taida_net_find_crlf(buf + body_offset + rp, 1048576);
        if (crlf < 0) return -1;

        // Parse hex chunk-size, ignoring chunk-ext
        size_t hex_end = (size_t)crlf;
        for (size_t i = 0; i < hex_end; i++) {
            if (buf[body_offset + rp + i] == ';') { hex_end = i; break; }
        }
        size_t hs = 0, he = hex_end;
        while (hs < he && (buf[body_offset + rp + hs] == ' ' || buf[body_offset + rp + hs] == '\t')) hs++;
        while (he > hs && (buf[body_offset + rp + he - 1] == ' ' || buf[body_offset + rp + he - 1] == '\t')) he--;
        if (hs >= he) return -1;

        // NB2-5: Reject oversized chunk-size to prevent overflow (parity with body_complete)
        if (he - hs > 15) return -1;
        size_t chunk_size = 0;
        for (size_t i = hs; i < he; i++) {
            unsigned char c = buf[body_offset + rp + i];
            int digit = -1;
            if (c >= '0' && c <= '9') digit = c - '0';
            else if (c >= 'a' && c <= 'f') digit = 10 + c - 'a';
            else if (c >= 'A' && c <= 'F') digit = 10 + c - 'A';
            if (digit < 0) return -1;
            chunk_size = chunk_size * 16 + (size_t)digit;
        }

        rp += (size_t)crlf + 2; // skip "size\r\n"

        if (chunk_size == 0) {
            // Skip trailers until final CRLF
            for (;;) {
                if (buf[body_offset + rp] == '\r' && buf[body_offset + rp + 1] == '\n') {
                    rp += 2;
                    break;
                }
                int64_t tc = taida_net_find_crlf(buf + body_offset + rp, 1048576);
                if (tc < 0) return -1;
                rp += (size_t)tc + 2;
            }
            out->body_len = wp;
            out->wire_consumed = rp;
            return 0;
        }

        // In-place copy using memmove (safe for overlapping regions)
        if (wp != rp) {
            memmove(buf + body_offset + wp, buf + body_offset + rp, chunk_size);
        }
        wp += chunk_size;
        rp += chunk_size;

        // Validate trailing CRLF
        if (buf[body_offset + rp] != '\r' || buf[body_offset + rp + 1] != '\n') return -1;
        rp += 2;
    }
}

// ── NET2-5: httpServe helper — build request pack ────────────────
static taida_val taida_net_build_request_pack(
    const unsigned char *raw_data, size_t raw_len,
    size_t body_start, size_t body_len, int64_t content_length,
    int is_chunked, int keep_alive,
    const char *remote_host, int remote_port
) {
    taida_val raw_bytes = taida_bytes_from_raw(raw_data, (taida_val)raw_len);

    // Parse to get spans
    taida_val parse_result = taida_net_http_parse_request_head(raw_bytes);
    taida_val inner = taida_pack_get(parse_result, taida_str_hash((taida_val)"__value"));

    taida_val request = taida_pack_new(13);
    taida_pack_set_hash(request, 0, taida_str_hash((taida_val)"raw"));
    taida_pack_set(request, 0, raw_bytes);
    taida_pack_set_tag(request, 0, TAIDA_TAG_PACK);  // Bytes
    taida_retain(raw_bytes);

    if (inner != 0 && taida_is_buchi_pack(inner)) {
        taida_val method_v = taida_pack_get(inner, taida_str_hash((taida_val)"method"));
        taida_pack_set_hash(request, 1, taida_str_hash((taida_val)"method"));
        taida_pack_set(request, 1, method_v);
        taida_pack_set_tag(request, 1, TAIDA_TAG_PACK);
        if (method_v > 4096) taida_retain(method_v);

        taida_val path_v = taida_pack_get(inner, taida_str_hash((taida_val)"path"));
        taida_pack_set_hash(request, 2, taida_str_hash((taida_val)"path"));
        taida_pack_set(request, 2, path_v);
        taida_pack_set_tag(request, 2, TAIDA_TAG_PACK);
        if (path_v > 4096) taida_retain(path_v);

        taida_val query_v = taida_pack_get(inner, taida_str_hash((taida_val)"query"));
        taida_pack_set_hash(request, 3, taida_str_hash((taida_val)"query"));
        taida_pack_set(request, 3, query_v);
        taida_pack_set_tag(request, 3, TAIDA_TAG_PACK);
        if (query_v > 4096) taida_retain(query_v);

        taida_val version_v = taida_pack_get(inner, taida_str_hash((taida_val)"version"));
        taida_pack_set_hash(request, 4, taida_str_hash((taida_val)"version"));
        taida_pack_set(request, 4, version_v);
        taida_pack_set_tag(request, 4, TAIDA_TAG_PACK);
        if (version_v > 4096) taida_retain(version_v);

        taida_val headers_v = taida_pack_get(inner, taida_str_hash((taida_val)"headers"));
        taida_pack_set_hash(request, 5, taida_str_hash((taida_val)"headers"));
        taida_pack_set(request, 5, headers_v);
        taida_pack_set_tag(request, 5, TAIDA_TAG_LIST);
        if (headers_v > 4096) taida_retain(headers_v);
    } else {
        taida_pack_set_hash(request, 1, taida_str_hash((taida_val)"method"));
        taida_pack_set(request, 1, taida_net_make_span(0, 0));
        taida_pack_set_tag(request, 1, TAIDA_TAG_PACK);
        taida_pack_set_hash(request, 2, taida_str_hash((taida_val)"path"));
        taida_pack_set(request, 2, taida_net_make_span(0, 0));
        taida_pack_set_tag(request, 2, TAIDA_TAG_PACK);
        taida_pack_set_hash(request, 3, taida_str_hash((taida_val)"query"));
        taida_pack_set(request, 3, taida_net_make_span(0, 0));
        taida_pack_set_tag(request, 3, TAIDA_TAG_PACK);
        taida_val ver = taida_pack_new(2);
        taida_pack_set_hash(ver, 0, taida_str_hash((taida_val)"major"));
        taida_pack_set(ver, 0, 1);
        taida_pack_set_hash(ver, 1, taida_str_hash((taida_val)"minor"));
        taida_pack_set(ver, 1, 1);
        taida_pack_set_hash(request, 4, taida_str_hash((taida_val)"version"));
        taida_pack_set(request, 4, ver);
        taida_pack_set_tag(request, 4, TAIDA_TAG_PACK);
        taida_pack_set_hash(request, 5, taida_str_hash((taida_val)"headers"));
        taida_pack_set(request, 5, taida_list_new());
        taida_pack_set_tag(request, 5, TAIDA_TAG_LIST);
    }

    taida_pack_set_hash(request, 6, taida_str_hash((taida_val)"body"));
    taida_pack_set(request, 6, taida_net_make_span((taida_val)body_start, (taida_val)body_len));
    taida_pack_set_tag(request, 6, TAIDA_TAG_PACK);

    taida_pack_set_hash(request, 7, taida_str_hash((taida_val)"bodyOffset"));
    taida_pack_set(request, 7, (taida_val)body_start);

    taida_pack_set_hash(request, 8, taida_str_hash((taida_val)"contentLength"));
    taida_pack_set(request, 8, (taida_val)content_length);

    taida_pack_set_hash(request, 9, taida_str_hash((taida_val)"remoteHost"));
    taida_pack_set(request, 9, (taida_val)taida_str_new_copy(remote_host));
    taida_pack_set_tag(request, 9, TAIDA_TAG_STR);

    taida_pack_set_hash(request, 10, taida_str_hash((taida_val)"remotePort"));
    taida_pack_set(request, 10, (taida_val)remote_port);

    taida_pack_set_hash(request, 11, taida_str_hash((taida_val)"keepAlive"));
    taida_pack_set(request, 11, keep_alive ? 1 : 0);
    taida_pack_set_tag(request, 11, TAIDA_TAG_BOOL);

    taida_pack_set_hash(request, 12, taida_str_hash((taida_val)"chunked"));
    taida_pack_set(request, 12, is_chunked ? 1 : 0);
    taida_pack_set_tag(request, 12, TAIDA_TAG_BOOL);

    return request;
}

// ── NET2-5: httpServe helper — send encoded response ─────────────
// NB2-20: Send directly from Bytes internal array — no extra malloc + byte-by-byte copy.
// Bytes layout: [header(magic+rc), length, byte0, byte1, ...] — each byte is a taida_val.
// We still need a contiguous buffer because taida_val slots are 8 bytes each (not 1 byte).
// Optimization: use stack buffer for small responses, heap only for large ones.
static void taida_net_send_response(int client_fd, taida_val encoded) {
    taida_val enc_throw = taida_pack_get(encoded, taida_str_hash((taida_val)"throw"));
    if (enc_throw == 0) {
        taida_val enc_inner = taida_pack_get(encoded, taida_str_hash((taida_val)"__value"));
        if (enc_inner != 0 && taida_is_buchi_pack(enc_inner)) {
            taida_val wire_bytes = taida_pack_get(enc_inner, taida_str_hash((taida_val)"bytes"));
            if (TAIDA_IS_BYTES(wire_bytes)) {
                taida_val *wb = (taida_val*)wire_bytes;
                taida_val wb_len = wb[1];
                // Use stack buffer for typical responses (< 4KB), heap for larger
                unsigned char stack_buf[4096];
                unsigned char *wb_buf;
                int heap_alloc = 0;
                if ((size_t)wb_len <= sizeof(stack_buf)) {
                    wb_buf = stack_buf;
                } else {
                    wb_buf = (unsigned char*)TAIDA_MALLOC((size_t)wb_len, "net_serve_send");
                    heap_alloc = 1;
                }
                for (taida_val i = 0; i < wb_len; i++) wb_buf[i] = (unsigned char)wb[2 + i];
                taida_net_send_all(client_fd, wb_buf, (size_t)wb_len);
                if (heap_alloc) free(wb_buf);
            }
        }
    } else {
        const char *fallback = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        taida_net_send_all(client_fd, fallback, strlen(fallback));
    }
}

// NB6-1: Scatter-gather send for internal one-shot response path.
// Builds head in one buffer, then sends head + body via writev (2 iovecs).
// Avoids the aggregate buffer concatenation of encode → materialize → send.
// Returns 0 on success, -1 on error.
static int taida_net_send_response_scatter(int client_fd, taida_val response) {
    if (!taida_is_buchi_pack(response)) return -1;

    taida_val status_hash = taida_str_hash((taida_val)"status");
    if (!taida_pack_has_hash(response, status_hash)) return -1;
    taida_val status = taida_pack_get(response, status_hash);
    if (status < 100 || status > 999) return -1;

    taida_val headers_hash = taida_str_hash((taida_val)"headers");
    if (!taida_pack_has_hash(response, headers_hash)) return -1;
    taida_val headers_ptr = taida_pack_get(response, headers_hash);
    if (!taida_is_list(headers_ptr)) return -1;

    taida_val body_hash = taida_str_hash((taida_val)"body");
    if (!taida_pack_has_hash(response, body_hash)) return -1;
    taida_val body_ptr = taida_pack_get(response, body_hash);

    // Determine body source and length.
    const unsigned char *body_data = NULL;
    taida_val *body_bytes_arr = NULL;
    size_t body_len = 0;
    int body_is_bytes = 0;
    // D29B-003 (Track-β, 2026-04-27): contig-bytes fast path. When the body
    // arrives as TAIDA_BYTES_CONTIG (e.g. from readBody/readBodyAll producing
    // contig form, or addon-side construction) we capture body_data directly
    // from the inline payload and skip the body_bytes_arr taida_val[] route
    // entirely. The downstream writev branch keys on body_is_contig to use
    // direct iovec reflection rather than the legacy byte-loop materialize.
    int body_is_contig = 0;

    if (TAIDA_IS_BYTES_CONTIG(body_ptr)) {
        body_data = taida_bytes_contig_data(body_ptr);
        taida_val blen = taida_bytes_contig_len(body_ptr);
        if (blen < 0) blen = 0;
        body_len = (size_t)blen;
        body_is_bytes = 1;
        body_is_contig = 1;
    } else if (TAIDA_IS_BYTES(body_ptr)) {
        body_bytes_arr = (taida_val*)body_ptr;
        taida_val blen = body_bytes_arr[1];
        if (blen < 0) blen = 0;
        body_len = (size_t)blen;
        body_is_bytes = 1;
    } else {
        size_t slen = 0;
        if (taida_read_cstr_len_safe((const char*)body_ptr, 10485760, &slen)) {
            body_data = (const unsigned char*)body_ptr;
            body_len = slen;
        } else {
            return -1;
        }
    }

    int no_body = (status >= 100 && status < 200) || status == 204 || status == 205 || status == 304;
    if (no_body && body_len > 0) return -1;

    // Build head buffer.
    char head_stack[2048];
    char *head = head_stack;
    size_t head_cap = sizeof(head_stack);
    size_t head_len = 0;

    const char *reason = taida_net_status_reason((int)status);
    head_len += (size_t)snprintf(head + head_len, head_cap - head_len,
                                  "HTTP/1.1 %d %s\r\n", (int)status, reason);

    taida_val name_hash = taida_str_hash((taida_val)"name");
    taida_val value_hash = taida_str_hash((taida_val)"value");
    int has_content_length = 0;

    taida_val *hlist = (taida_val*)headers_ptr;
    taida_val hcount = hlist[2];
    for (taida_val i = 0; i < hcount; i++) {
        taida_val hdr = hlist[4 + i];
        if (!taida_is_buchi_pack(hdr)) { if (head != head_stack) free(head); return -1; }
        taida_val hname = taida_pack_get(hdr, name_hash);
        taida_val hvalue = taida_pack_get(hdr, value_hash);
        const char *hname_s = (const char*)hname;
        const char *hvalue_s = (const char*)hvalue;
        size_t hn_len = 0, hv_len = 0;
        if (!taida_read_cstr_len_safe(hname_s, 8192, &hn_len)) { if (head != head_stack) free(head); return -1; }
        if (!taida_read_cstr_len_safe(hvalue_s, 65536, &hv_len)) { if (head != head_stack) free(head); return -1; }

        // NB-13: Reject CRLF in header name/value (parity with public encoder)
        for (size_t k = 0; k < hn_len; k++) {
            if (hname_s[k] == '\r' || hname_s[k] == '\n') { if (head != head_stack) free(head); return -1; }
        }
        for (size_t k = 0; k < hv_len; k++) {
            if (hvalue_s[k] == '\r' || hvalue_s[k] == '\n') { if (head != head_stack) free(head); return -1; }
        }

        // Check content-length
        if (hn_len == 14) {
            const char *cl_expected = "content-length";
            int is_cl = 1;
            for (size_t k = 0; k < 14; k++) {
                char c = hname_s[k];
                if (c >= 'A' && c <= 'Z') c += 32;
                if (c != cl_expected[k]) { is_cl = 0; break; }
            }
            if (is_cl) {
                if (no_body) continue;
                has_content_length = 1;
            }
        }

        size_t needed = head_len + hn_len + hv_len + 4;
        if (needed > head_cap) {
            head_cap = needed * 2;
            if (head == head_stack) {
                head = (char*)TAIDA_MALLOC(head_cap, "net_scatter_head");
                memcpy(head, head_stack, head_len);
            } else {
                TAIDA_REALLOC(head, head_cap, "net_scatter_head");
            }
        }
        memcpy(head + head_len, hname_s, hn_len); head_len += hn_len;
        head[head_len++] = ':'; head[head_len++] = ' ';
        memcpy(head + head_len, hvalue_s, hv_len); head_len += hv_len;
        head[head_len++] = '\r'; head[head_len++] = '\n';
    }

    if (!no_body && !has_content_length) {
        char cl_hdr[64];
        int cl_len = snprintf(cl_hdr, sizeof(cl_hdr), "Content-Length: %zu\r\n", body_len);
        size_t needed = head_len + (size_t)cl_len;
        if (needed > head_cap) {
            head_cap = needed * 2;
            if (head == head_stack) {
                head = (char*)TAIDA_MALLOC(head_cap, "net_scatter_head");
                memcpy(head, head_stack, head_len);
            } else {
                TAIDA_REALLOC(head, head_cap, "net_scatter_head");
            }
        }
        memcpy(head + head_len, cl_hdr, (size_t)cl_len);
        head_len += (size_t)cl_len;
    }

    // End of headers.
    if (head_len + 2 > head_cap) {
        head_cap = head_len + 2;
        if (head == head_stack) {
            head = (char*)TAIDA_MALLOC(head_cap, "net_scatter_head");
            memcpy(head, head_stack, head_len);
        } else {
            TAIDA_REALLOC(head, head_cap, "net_scatter_head");
        }
    }
    head[head_len++] = '\r'; head[head_len++] = '\n';

    // Send using scatter-gather (writev).
    int rc;
    if (no_body || body_len == 0) {
        rc = taida_net_send_all(client_fd, head, head_len);
    } else if (!body_is_bytes) {
        // Str body: already contiguous, use 2 iovecs.
        struct iovec iov[2];
        iov[0].iov_base = head;
        iov[0].iov_len = head_len;
        iov[1].iov_base = (void*)body_data;
        iov[1].iov_len = body_len;
        rc = taida_tls_writev_all(client_fd, iov, 2);
    } else if (body_is_contig) {
        // D29B-003 (Track-β, 2026-04-27): contig-bytes fast path. body_data
        // already points at the inline contiguous payload of the
        // TAIDA_BYTES_CONTIG header, so we reflect it directly into iov[1]
        // — no per-byte materialize, no intermediate buffer alloc, true
        // payload-level zero-copy from handler-provided Bytes through
        // writev() into the kernel.
        struct iovec iov[2];
        iov[0].iov_base = head;
        iov[0].iov_len = head_len;
        iov[1].iov_base = (void*)body_data;
        iov[1].iov_len = body_len;
        rc = taida_tls_writev_all(client_fd, iov, 2);
    } else {
        // Legacy TAIDA_BYTES_MAGIC body: materialize from taida_val array into
        // a contiguous buffer once, then send head + body via 2 iovecs. The
        // legacy path is preserved for backward compatibility with handlers
        // that build Bytes through the historical taida_val[] route. New
        // producers (readBody, readBodyAll) emit TAIDA_BYTES_CONTIG so this
        // branch should be cold under the D29 hot path.
        unsigned char body_stack[4096];
        unsigned char *body_buf = (body_len <= sizeof(body_stack)) ? body_stack
            : (unsigned char*)TAIDA_MALLOC(body_len, "net_scatter_body");
        for (size_t i = 0; i < body_len; i++) {
            body_buf[i] = (unsigned char)body_bytes_arr[2 + i];
        }
        struct iovec iov[2];
        iov[0].iov_base = head;
        iov[0].iov_len = head_len;
        iov[1].iov_base = body_buf;
        iov[1].iov_len = body_len;
        rc = taida_tls_writev_all(client_fd, iov, 2);
        if (body_buf != body_stack) free(body_buf);
    }

    if (head != head_stack) free(head);
    return rc;
}

// ── NET3-5a/5b/5c/5d/5e: v3 streaming writer state machine ─────────────
// Writer state: Idle(0) → HeadPrepared(1) → Streaming(2) → Ended(3)
// Thread-local context for v3 streaming API. Set in the worker thread
// before invoking a 2-arg handler; the v3 API functions (startResponse,
// writeChunk, endResponse, sseEvent) access it via these thread-locals.

#define NET3_STATE_IDLE         0
#define NET3_STATE_HEAD_PREPARED 1
#define NET3_STATE_STREAMING    2
#define NET3_STATE_ENDED        3
#define NET3_STATE_WEBSOCKET    4

// Maximum pending headers per streaming response
#define NET3_MAX_HEADERS 64

typedef struct {
    int state;               // NET3_STATE_*
    int pending_status;      // default 200
    int sse_mode;            // SSE auto-headers applied
    int header_count;        // number of pending headers
    // Stack-allocated header storage (no per-request malloc for headers)
    const char *header_names[NET3_MAX_HEADERS];
    const char *header_values[NET3_MAX_HEADERS];
} Net3WriterState;

// ── v4 Request Body Streaming State ──────────────────────────
// Per-request state for body-deferred 2-arg handlers.
// Lives on the worker stack; v4 API functions access it via thread-local.

#define NET4_CHUNKED_WAIT_SIZE    0
#define NET4_CHUNKED_READ_DATA    1
#define NET4_CHUNKED_WAIT_TRAILER 2
#define NET4_CHUNKED_DONE         3

typedef struct {
    int is_chunked;          // Transfer-Encoding: chunked?
    int64_t content_length;  // Content-Length from head (0 if absent/chunked)
    int64_t bytes_consumed;  // how many body bytes consumed so far (CL path)
    int fully_read;          // body fully consumed?
    int any_read_started;    // any readBodyChunk/readBodyAll call made?
    // Leftover bytes from head parsing that are body bytes already received.
    unsigned char *leftover;
    size_t leftover_len;
    size_t leftover_pos;     // current position within leftover
    // Chunked decoder state
    int chunked_state;       // NET4_CHUNKED_*
    size_t chunked_remaining;// bytes remaining in current chunk
    // Request-scoped identity token (NB4-7 parity)
    uint64_t request_token;
    // WebSocket close state
    int ws_closed;
    // NB4-10: Connection-scoped WebSocket token for identity verification.
    uint64_t ws_token;
    // v5: Received close code from peer's close frame (0 = not received).
    int64_t ws_close_code;
} Net4BodyState;

// Global monotonic counter for unique request tokens (NB4-7 parity).
static volatile uint64_t taida_net4_next_token = 1;
static uint64_t taida_net4_alloc_token(void) {
    return __atomic_fetch_add(&taida_net4_next_token, 1, __ATOMIC_RELAXED);
}

// NB4-10: Global monotonic counter for unique WebSocket connection tokens.
static volatile uint64_t taida_net4_next_ws_token = 1;
static uint64_t taida_net4_alloc_ws_token(void) {
    return __atomic_fetch_add(&taida_net4_next_ws_token, 1, __ATOMIC_RELAXED);
}

// Thread-local: current writer state and client fd for v3 streaming API.
// These are set/cleared around each 2-arg handler invocation.
static __thread Net3WriterState *tl_net3_writer = NULL;
static __thread int tl_net3_client_fd = -1;
// v4: per-request body streaming state for 2-arg handlers.
static __thread Net4BodyState *tl_net4_body = NULL;

// Forward declaration: writer token validation (defined after create_writer_token).
static void taida_net3_validate_writer(taida_val writer, const char *api_name);

// NET3-5c: writev()-based send helper. Sends all iov buffers, handling
// partial writes and EINTR. Returns 0 on success, -1 on error.
// NET5-4a: Routes through TLS when tl_ssl is active.
static int taida_net_writev_all(int fd, struct iovec *iov, int iovcnt) {
    return taida_tls_writev_all(fd, iov, iovcnt);
}

// Check if a status code forbids a message body (1xx, 204, 205, 304).
static int taida_net3_is_bodyless_status(int status) {
    return (status >= 100 && status <= 199) || status == 204 || status == 205 || status == 304;
}

// Build and send the streaming response head.
// Appends Transfer-Encoding: chunked for non-bodyless status codes.
// Uses stack buffer (no per-request malloc for typical headers).
// Returns 0 on success, -1 on send error, -2 on head overflow.
#define NET3_HEAD_BUF_SIZE 8192
static int taida_net3_commit_head(int fd, Net3WriterState *w) {
    char head_buf[NET3_HEAD_BUF_SIZE];
    size_t cap = sizeof(head_buf);
    size_t offset = 0;
    int n;

    const char *reason = taida_net_status_reason(w->pending_status);
    n = snprintf(head_buf, cap, "HTTP/1.1 %d %s\r\n", w->pending_status, reason);
    if (n < 0 || (size_t)n >= cap) goto overflow;
    offset += (size_t)n;

    for (int i = 0; i < w->header_count && i < NET3_MAX_HEADERS; i++) {
        size_t remaining = cap - offset;
        n = snprintf(head_buf + offset, remaining,
                     "%s: %s\r\n", w->header_names[i], w->header_values[i]);
        if (n < 0 || (size_t)n >= remaining) goto overflow;
        offset += (size_t)n;
    }
    if (!taida_net3_is_bodyless_status(w->pending_status)) {
        size_t remaining = cap - offset;
        n = snprintf(head_buf + offset, remaining, "Transfer-Encoding: chunked\r\n");
        if (n < 0 || (size_t)n >= remaining) goto overflow;
        offset += (size_t)n;
    }
    {
        size_t remaining = cap - offset;
        n = snprintf(head_buf + offset, remaining, "\r\n");
        if (n < 0 || (size_t)n >= remaining) goto overflow;
        offset += (size_t)n;
    }
    return taida_net_send_all(fd, head_buf, offset);

overflow:
    fprintf(stderr, "commit_head: response head exceeds %d bytes (too many or too large headers)\n",
            (int)NET3_HEAD_BUF_SIZE);
    return -2;
}

// Validate reserved headers (Content-Length, Transfer-Encoding) in streaming path.
// Returns 0 if valid, prints error to stderr and returns -1 if invalid.
static int taida_net3_validate_reserved_headers(taida_val headers, const char *api_name) {
    if (!TAIDA_IS_LIST(headers)) return 0;
    taida_val *list = (taida_val*)headers;
    taida_val len = list[2];
    for (taida_val i = 0; i < len; i++) {
        taida_val item = list[4 + i];
        if (!taida_is_buchi_pack(item)) continue;
        taida_val name_val = taida_pack_get(item, taida_str_hash((taida_val)"name"));
        if (name_val == 0) continue;
        const char *name_str = (const char*)name_val;
        size_t name_len = 0;
        if (!taida_read_cstr_len_safe(name_str, 256, &name_len)) continue;
        // Case-insensitive comparison
        if (name_len == 14) {
            // "content-length" (14 chars)
            char lower[15];
            for (size_t j = 0; j < name_len; j++) lower[j] = (char)((name_str[j] >= 'A' && name_str[j] <= 'Z') ? name_str[j] + 32 : name_str[j]);
            lower[name_len] = '\0';
            if (strcmp(lower, "content-length") == 0) {
                fprintf(stderr, "%s: 'Content-Length' is not allowed in streaming response headers. "
                        "The runtime manages Content-Length/Transfer-Encoding for streaming responses.\n", api_name);
                return -1;
            }
        }
        if (name_len == 17) {
            // "transfer-encoding" (17 chars)
            char lower[18];
            for (size_t j = 0; j < name_len; j++) lower[j] = (char)((name_str[j] >= 'A' && name_str[j] <= 'Z') ? name_str[j] + 32 : name_str[j]);
            lower[name_len] = '\0';
            if (strcmp(lower, "transfer-encoding") == 0) {
                fprintf(stderr, "%s: 'Transfer-Encoding' is not allowed in streaming response headers. "
                        "The runtime manages Transfer-Encoding for streaming responses.\n", api_name);
                return -1;
            }
        }
    }
    return 0;
}

// Extract headers from a taida list of @(name, value) packs into the writer state.
static void taida_net3_extract_headers(Net3WriterState *w, taida_val headers) {
    w->header_count = 0;
    if (!TAIDA_IS_LIST(headers)) return;
    taida_val *list = (taida_val*)headers;
    taida_val len = list[2];
    for (taida_val i = 0; i < len && w->header_count < NET3_MAX_HEADERS; i++) {
        taida_val item = list[4 + i];
        if (!taida_is_buchi_pack(item)) continue;
        taida_val name_val = taida_pack_get(item, taida_str_hash((taida_val)"name"));
        taida_val value_val = taida_pack_get(item, taida_str_hash((taida_val)"value"));
        if (name_val == 0 || value_val == 0) continue;
        w->header_names[w->header_count] = (const char*)name_val;
        w->header_values[w->header_count] = (const char*)value_val;
        w->header_count++;
    }
}

// NET3-5b: startResponse(writer, status, headers)
// Updates pending status/headers on the writer state. Does NOT commit to wire.
taida_val taida_net_start_response(taida_val writer, taida_val status, taida_val headers) {
    taida_net3_validate_writer(writer, "startResponse");
    Net3WriterState *w = tl_net3_writer;
    if (!w) {
        fprintf(stderr, "startResponse: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }
    // State check
    switch (w->state) {
        case NET3_STATE_IDLE: break;
        case NET3_STATE_HEAD_PREPARED:
            fprintf(stderr, "startResponse: already called. Cannot call startResponse twice.\n");
            exit(1);
        case NET3_STATE_STREAMING:
            fprintf(stderr, "startResponse: head already committed (chunks are being written). Cannot change status/headers after writeChunk.\n");
            exit(1);
        case NET3_STATE_ENDED:
            fprintf(stderr, "startResponse: response already ended.\n");
            exit(1);
    }
    // Validate status range
    if (status < 100 || status > 599) {
        fprintf(stderr, "startResponse: status must be 100-599, got %lld\n", (long long)status);
        exit(1);
    }
    // Validate reserved headers
    if (taida_net3_validate_reserved_headers(headers, "startResponse") < 0) {
        exit(1);
    }
    w->pending_status = (int)status;
    taida_net3_extract_headers(w, headers);
    w->state = NET3_STATE_HEAD_PREPARED;
    return 0; // Unit
}

// NET3-5b/5c/5d: writeChunk(writer, data)
// Sends one chunk of body data using chunked TE. Uses writev() for zero-copy.
// Bytes: extract from taida_val array to stack/stack-heap buffer, then writev.
// Str: use C string directly.
taida_val taida_net_write_chunk(taida_val writer, taida_val data) {
    taida_net3_validate_writer(writer, "writeChunk");
    Net3WriterState *w = tl_net3_writer;
    int fd = tl_net3_client_fd;
    if (!w) {
        fprintf(stderr, "writeChunk: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }
    if (w->state == NET3_STATE_ENDED) {
        fprintf(stderr, "writeChunk: response already ended.\n");
        exit(1);
    }

    // Extract payload pointer and length
    const unsigned char *payload = NULL;
    size_t payload_len = 0;
    // NET3-5d: For legacy Bytes (TAIDA_BYTES_MAGIC), we need to convert from
    // taida_val array to contiguous bytes. Use stack buffer for small payloads,
    // heap only for large ones. No per-chunk persistent alloc.
    // D29B-003 (Track-β): For TAIDA_BYTES_CONTIG, payload reflects the inline
    // contig buffer directly — zero allocation, true payload-level zero-copy.
    unsigned char stack_payload[4096];
    unsigned char *heap_payload = NULL;
    int is_bytes = 0;

    if (TAIDA_IS_BYTES_CONTIG(data)) {
        // D29B-003 (Track-β, 2026-04-27): contig fast path. Direct pointer
        // reflection into payload — no taida_val[] byte-loop, no temporary
        // stack/heap buffer alloc. iov[1].iov_base will land on the inline
        // contiguous payload owned by the Bytes header.
        is_bytes = 1;
        payload = taida_bytes_contig_data(data);
        taida_val blen = taida_bytes_contig_len(data);
        payload_len = (size_t)blen;
        if (payload_len == 0) return 0; // empty chunk is no-op
    } else if (TAIDA_IS_BYTES(data)) {
        // Legacy taida_val[] Bytes form: materialize once via byte-loop. New
        // producers (readBody/readBodyAll) emit TAIDA_BYTES_CONTIG so this
        // branch is cold in the D29 hot path.
        is_bytes = 1;
        taida_val *bytes = (taida_val*)data;
        taida_val blen = bytes[1];
        payload_len = (size_t)blen;
        if (payload_len == 0) return 0; // empty chunk is no-op
        if (payload_len <= sizeof(stack_payload)) {
            for (size_t i = 0; i < payload_len; i++) stack_payload[i] = (unsigned char)bytes[2 + i];
            payload = stack_payload;
        } else {
            heap_payload = (unsigned char*)TAIDA_MALLOC(payload_len, "net3_write_chunk_bytes");
            for (size_t i = 0; i < payload_len; i++) heap_payload[i] = (unsigned char)bytes[2 + i];
            payload = heap_payload;
        }
    } else {
        // Assume Str (C string)
        const char *str = (const char*)data;
        size_t slen = 0;
        if (!taida_read_cstr_len_safe(str, 16 * 1024 * 1024, &slen)) {
            fprintf(stderr, "writeChunk: data must be Bytes or Str\n");
            if (heap_payload) free(heap_payload);
            exit(1);
        }
        payload = (const unsigned char*)str;
        payload_len = slen;
        if (payload_len == 0) return 0; // empty chunk is no-op
    }

    // Bodyless status check
    if (taida_net3_is_bodyless_status(w->pending_status)) {
        fprintf(stderr, "writeChunk: status %d does not allow a message body\n", w->pending_status);
        if (heap_payload) free(heap_payload);
        exit(1);
    }

    // Commit head if not yet committed
    if (w->state == NET3_STATE_IDLE || w->state == NET3_STATE_HEAD_PREPARED) {
        if (taida_net3_commit_head(fd, w) != 0) {
            fprintf(stderr, "writeChunk: failed to commit response head\n");
            if (heap_payload) free(heap_payload);
            exit(1);
        }
        w->state = NET3_STATE_STREAMING;
    }

    // NET3-5c: Send chunk using writev() — zero-copy for payload.
    // Wire format: <hex-size>\r\n<payload>\r\n
    char hex_prefix[32];
    int hex_len = snprintf(hex_prefix, sizeof(hex_prefix), "%zx\r\n", payload_len);

    struct iovec iov[3];
    iov[0].iov_base = hex_prefix;
    iov[0].iov_len = (size_t)hex_len;
    iov[1].iov_base = (void*)payload;
    iov[1].iov_len = payload_len;
    iov[2].iov_base = (void*)"\r\n";
    iov[2].iov_len = 2;

    // NB3-5: Check writev_all return value for write errors (e.g. peer RST).
    if (taida_net_writev_all(fd, iov, 3) != 0) {
        if (heap_payload) free(heap_payload);
        fprintf(stderr, "writeChunk: failed to send chunk data\n");
        exit(1);
    }

    if (heap_payload) free(heap_payload);
    return 0; // Unit
}

// NET3-5b: endResponse(writer)
// Terminates the chunked response by sending 0\r\n\r\n.
// Idempotent: second call is a no-op.
taida_val taida_net_end_response(taida_val writer) {
    taida_net3_validate_writer(writer, "endResponse");
    Net3WriterState *w = tl_net3_writer;
    int fd = tl_net3_client_fd;
    if (!w) {
        fprintf(stderr, "endResponse: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }
    // Idempotent: no-op if already ended
    if (w->state == NET3_STATE_ENDED) return 0;

    // Commit head if not yet committed
    if (w->state == NET3_STATE_IDLE || w->state == NET3_STATE_HEAD_PREPARED) {
        if (taida_net3_commit_head(fd, w) != 0) {
            fprintf(stderr, "endResponse: failed to commit response head\n");
            exit(1);
        }
    }

    // Send chunked terminator — but only for non-bodyless status
    if (!taida_net3_is_bodyless_status(w->pending_status)) {
        taida_net_send_all(fd, "0\r\n\r\n", 5);
    }
    w->state = NET3_STATE_ENDED;
    return 0; // Unit
}

// NET3-5e: sseEvent(writer, event, data)
// SSE convenience API. Sends one Server-Sent Event.
// Auto-sets Content-Type and Cache-Control headers if not already set.
// Splits multiline data into data: lines.
taida_val taida_net_sse_event(taida_val writer, taida_val event, taida_val data) {
    taida_net3_validate_writer(writer, "sseEvent");
    Net3WriterState *w = tl_net3_writer;
    int fd = tl_net3_client_fd;
    if (!w) {
        fprintf(stderr, "sseEvent: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }
    // Validate event and data are strings.
    // NB3-8: Use taida_str_byte_len which reads heap string length from header
    // metadata instead of scanning for NUL. This is correct for non-ASCII
    // (multi-byte UTF-8) strings and avoids parity issues with Interpreter/JS.
    const char *event_str = (const char*)event;
    const char *data_str = (const char*)data;
    size_t event_len = 0, data_len = 0;
    if (!taida_str_byte_len(event_str, &event_len)) {
        fprintf(stderr, "sseEvent: event must be Str\n");
        exit(1);
    }
    if (!taida_str_byte_len(data_str, &data_len)) {
        fprintf(stderr, "sseEvent: data must be Str\n");
        exit(1);
    }

    if (w->state == NET3_STATE_ENDED) {
        fprintf(stderr, "sseEvent: response already ended.\n");
        exit(1);
    }
    if (taida_net3_is_bodyless_status(w->pending_status)) {
        fprintf(stderr, "sseEvent: status %d does not allow a message body\n", w->pending_status);
        exit(1);
    }

    // SSE auto-headers (once per writer)
    if (!w->sse_mode) {
        if (w->state == NET3_STATE_STREAMING) {
            // Head already committed — check if SSE headers were set
            int has_ct = 0, has_cc = 0;
            for (int i = 0; i < w->header_count; i++) {
                const char *n = w->header_names[i];
                size_t nlen = 0;
                if (!taida_read_cstr_len_safe(n, 256, &nlen)) continue;
                // Case-insensitive check
                if (nlen == 12) {
                    char lower[13];
                    for (size_t j = 0; j < nlen; j++) lower[j] = (char)((n[j] >= 'A' && n[j] <= 'Z') ? n[j] + 32 : n[j]);
                    lower[nlen] = '\0';
                    if (strcmp(lower, "content-type") == 0) {
                        const char *v = w->header_values[i];
                        size_t vlen = 0;
                        if (taida_read_cstr_len_safe(v, 256, &vlen)) {
                            char lv[256];
                            for (size_t j = 0; j < vlen && j < 255; j++) lv[j] = (char)((v[j] >= 'A' && v[j] <= 'Z') ? v[j] + 32 : v[j]);
                            lv[vlen < 255 ? vlen : 255] = '\0';
                            if (strstr(lv, "text/event-stream")) has_ct = 1;
                        }
                    }
                }
                if (nlen == 13) {
                    char lower[14];
                    for (size_t j = 0; j < nlen; j++) lower[j] = (char)((n[j] >= 'A' && n[j] <= 'Z') ? n[j] + 32 : n[j]);
                    lower[nlen] = '\0';
                    if (strcmp(lower, "cache-control") == 0) {
                        const char *v = w->header_values[i];
                        size_t vlen = 0;
                        if (taida_read_cstr_len_safe(v, 256, &vlen)) {
                            char lv[256];
                            for (size_t j = 0; j < vlen && j < 255; j++) lv[j] = (char)((v[j] >= 'A' && v[j] <= 'Z') ? v[j] + 32 : v[j]);
                            lv[vlen < 255 ? vlen : 255] = '\0';
                            if (strstr(lv, "no-cache")) has_cc = 1;
                        }
                    }
                }
            }
            if (!has_ct || !has_cc) {
                fprintf(stderr, "sseEvent: head already committed without SSE headers. "
                        "Call sseEvent before writeChunk, or use startResponse "
                        "with explicit Content-Type: text/event-stream and "
                        "Cache-Control: no-cache headers before writeChunk.\n");
                exit(1);
            }
            w->sse_mode = 1;
        } else {
            // Head not yet committed — safe to add auto-headers
            int has_ct = 0, has_cc = 0;
            for (int i = 0; i < w->header_count; i++) {
                const char *n = w->header_names[i];
                size_t nlen = 0;
                if (!taida_read_cstr_len_safe(n, 256, &nlen)) continue;
                char lower[256];
                for (size_t j = 0; j < nlen && j < 255; j++) lower[j] = (char)((n[j] >= 'A' && n[j] <= 'Z') ? n[j] + 32 : n[j]);
                lower[nlen < 255 ? nlen : 255] = '\0';
                if (strcmp(lower, "content-type") == 0) has_ct = 1;
                if (strcmp(lower, "cache-control") == 0) has_cc = 1;
            }
            if (!has_ct && w->header_count < NET3_MAX_HEADERS) {
                w->header_names[w->header_count] = "Content-Type";
                w->header_values[w->header_count] = "text/event-stream; charset=utf-8";
                w->header_count++;
            }
            if (!has_cc && w->header_count < NET3_MAX_HEADERS) {
                w->header_names[w->header_count] = "Cache-Control";
                w->header_values[w->header_count] = "no-cache";
                w->header_count++;
            }
            w->sse_mode = 1;
        }
    }

    // Commit head if not yet committed
    if (w->state == NET3_STATE_IDLE || w->state == NET3_STATE_HEAD_PREPARED) {
        if (taida_net3_commit_head(fd, w) != 0) {
            fprintf(stderr, "sseEvent: failed to commit response head\n");
            exit(1);
        }
        w->state = NET3_STATE_STREAMING;
    }

    // Build SSE event payload and compute total length.
    // Wire format:
    //   event: <event>\n      (omit if event is empty)
    //   data: <line1>\n
    //   data: <line2>\n       (for each line in data split by \n)
    //   \n                    (event terminator)

    // Count data lines
    int line_count = 1;
    for (size_t i = 0; i < data_len; i++) {
        if (data_str[i] == '\n') line_count++;
    }

    // Compute total payload length for chunk header
    size_t total_payload = 0;
    if (event_len > 0) {
        total_payload += 7 + event_len + 1; // "event: " + event + "\n"
    }
    // For each data line: "data: " + line + "\n"
    {
        const char *p = data_str;
        const char *end = data_str + data_len;
        while (p <= end) {
            const char *nl = p;
            while (nl < end && *nl != '\n') nl++;
            size_t line_len = (size_t)(nl - p);
            total_payload += 6 + line_len + 1; // "data: " + line + "\n"
            p = nl + 1;
            if (nl == end) break;
        }
    }
    total_payload += 1; // terminator "\n"

    // Build chunk: hex_prefix + SSE payload + chunk suffix
    char hex_prefix[32];
    int hex_len = snprintf(hex_prefix, sizeof(hex_prefix), "%zx\r\n", total_payload);

    // Use iov array. Max iovecs: 1(hex) + 3(event line) + 3*line_count(data lines) + 1(term) + 1(suffix)
    int max_iov = 1 + 3 + 3 * line_count + 1 + 1;
    // Use stack for small SSE events, heap for large
    struct iovec stack_iov[64];
    struct iovec *iov = (max_iov <= 64) ? stack_iov : (struct iovec*)TAIDA_MALLOC(sizeof(struct iovec) * (size_t)max_iov, "net3_sse_iov");
    int iov_count = 0;

    // hex prefix
    iov[iov_count].iov_base = hex_prefix;
    iov[iov_count].iov_len = (size_t)hex_len;
    iov_count++;

    // event: line
    if (event_len > 0) {
        iov[iov_count].iov_base = (void*)"event: ";
        iov[iov_count].iov_len = 7;
        iov_count++;
        iov[iov_count].iov_base = (void*)event_str;
        iov[iov_count].iov_len = event_len;
        iov_count++;
        iov[iov_count].iov_base = (void*)"\n";
        iov[iov_count].iov_len = 1;
        iov_count++;
    }

    // data: lines
    {
        const char *p = data_str;
        const char *end = data_str + data_len;
        while (p <= end) {
            const char *nl = p;
            while (nl < end && *nl != '\n') nl++;
            size_t line_len = (size_t)(nl - p);
            iov[iov_count].iov_base = (void*)"data: ";
            iov[iov_count].iov_len = 6;
            iov_count++;
            if (line_len > 0) {
                iov[iov_count].iov_base = (void*)p;
                iov[iov_count].iov_len = line_len;
                iov_count++;
            }
            iov[iov_count].iov_base = (void*)"\n";
            iov[iov_count].iov_len = 1;
            iov_count++;
            p = nl + 1;
            if (nl == end) break;
        }
    }

    // event terminator
    iov[iov_count].iov_base = (void*)"\n";
    iov[iov_count].iov_len = 1;
    iov_count++;

    // chunk suffix
    iov[iov_count].iov_base = (void*)"\r\n";
    iov[iov_count].iov_len = 2;
    iov_count++;

    // NB3-5: Check writev_all return value for write errors (e.g. peer RST).
    if (taida_net_writev_all(fd, iov, iov_count) != 0) {
        if (iov != stack_iov) free(iov);
        fprintf(stderr, "sseEvent: failed to send SSE chunk data\n");
        exit(1);
    }

    if (iov != stack_iov) free(iov);

    return 0; // Unit
}

// ── NET4-4: v4 Request Body Streaming + WebSocket — Native backend ──
//
// Phase 4: Full implementation of readBodyChunk, readBodyAll,
// wsUpgrade, wsSend, wsReceive, wsClose.
// Replaces NB4-6 stubs.

// ── SHA-1 implementation (RFC 3174, ~100 lines) ─────────────
// Used exclusively for WebSocket Sec-WebSocket-Accept calculation.
// Not for cryptographic purposes.

static void taida_sha1_transform(uint32_t state[5], const uint8_t block[64]) {
    uint32_t w[80];
    for (int i = 0; i < 16; i++) {
        w[i] = ((uint32_t)block[i*4] << 24) | ((uint32_t)block[i*4+1] << 16)
             | ((uint32_t)block[i*4+2] << 8) | (uint32_t)block[i*4+3];
    }
    for (int i = 16; i < 80; i++) {
        uint32_t t = w[i-3] ^ w[i-8] ^ w[i-14] ^ w[i-16];
        w[i] = (t << 1) | (t >> 31);
    }
    uint32_t a = state[0], b = state[1], c = state[2], d = state[3], e = state[4];
    for (int i = 0; i < 80; i++) {
        uint32_t f, k;
        if (i < 20)      { f = (b & c) | ((~b) & d); k = 0x5A827999; }
        else if (i < 40) { f = b ^ c ^ d;             k = 0x6ED9EBA1; }
        else if (i < 60) { f = (b & c) | (b & d) | (c & d); k = 0x8F1BBCDC; }
        else              { f = b ^ c ^ d;             k = 0xCA62C1D6; }
        uint32_t temp = ((a << 5) | (a >> 27)) + f + e + k + w[i];
        e = d; d = c; c = (b << 30) | (b >> 2); b = a; a = temp;
    }
    state[0] += a; state[1] += b; state[2] += c; state[3] += d; state[4] += e;
}

// SHA-1 hash: input -> 20-byte digest.
static void taida_sha1(const uint8_t *data, size_t len, uint8_t digest[20]) {
    uint32_t state[5] = { 0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0 };
    size_t i;
    uint8_t block[64];
    size_t block_pos = 0;

    for (i = 0; i < len; i++) {
        block[block_pos++] = data[i];
        if (block_pos == 64) {
            taida_sha1_transform(state, block);
            block_pos = 0;
        }
    }

    // Padding
    block[block_pos++] = 0x80;
    if (block_pos > 56) {
        while (block_pos < 64) block[block_pos++] = 0;
        taida_sha1_transform(state, block);
        block_pos = 0;
    }
    while (block_pos < 56) block[block_pos++] = 0;

    // Length in bits (big-endian 64-bit)
    uint64_t bit_len = (uint64_t)len * 8;
    block[56] = (uint8_t)(bit_len >> 56);
    block[57] = (uint8_t)(bit_len >> 48);
    block[58] = (uint8_t)(bit_len >> 40);
    block[59] = (uint8_t)(bit_len >> 32);
    block[60] = (uint8_t)(bit_len >> 24);
    block[61] = (uint8_t)(bit_len >> 16);
    block[62] = (uint8_t)(bit_len >> 8);
    block[63] = (uint8_t)(bit_len);
    taida_sha1_transform(state, block);

    for (i = 0; i < 5; i++) {
        digest[i*4]   = (uint8_t)(state[i] >> 24);
        digest[i*4+1] = (uint8_t)(state[i] >> 16);
        digest[i*4+2] = (uint8_t)(state[i] >> 8);
        digest[i*4+3] = (uint8_t)(state[i]);
    }
}

// ── Base64 encode ──────────────────────────────────────────
static const char taida_b64_chars[] =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

// Base64 encode: input bytes -> null-terminated string (caller must free).
static char *taida_base64_encode(const uint8_t *data, size_t len) {
    size_t out_len = 4 * ((len + 2) / 3);
    char *out = (char*)TAIDA_MALLOC(out_len + 1, "net_base64");
    size_t j = 0;
    for (size_t i = 0; i < len; ) {
        uint32_t octet_a = i < len ? data[i++] : 0;
        uint32_t octet_b = i < len ? data[i++] : 0;
        uint32_t octet_c = i < len ? data[i++] : 0;
        uint32_t triple = (octet_a << 16) | (octet_b << 8) | octet_c;
        out[j++] = taida_b64_chars[(triple >> 18) & 0x3F];
        out[j++] = taida_b64_chars[(triple >> 12) & 0x3F];
        out[j++] = taida_b64_chars[(triple >> 6) & 0x3F];
        out[j++] = taida_b64_chars[triple & 0x3F];
    }
    // Padding
    size_t mod = len % 3;
    if (mod == 1) { out[j-1] = '='; out[j-2] = '='; }
    else if (mod == 2) { out[j-1] = '='; }
    out[j] = '\0';
    return out;
}

// NB4-11: Base64 decode for Sec-WebSocket-Key validation.
// Returns decoded length, or -1 on invalid input. Writes to `out` (must have enough space).
static int taida_base64_decode(const char *input, size_t input_len, uint8_t *out, size_t out_cap) {
    static const int8_t decode_table[256] = {
        [0 ... 255] = -1,
        ['A'] = 0, ['B'] = 1, ['C'] = 2, ['D'] = 3, ['E'] = 4, ['F'] = 5,
        ['G'] = 6, ['H'] = 7, ['I'] = 8, ['J'] = 9, ['K'] = 10, ['L'] = 11,
        ['M'] = 12, ['N'] = 13, ['O'] = 14, ['P'] = 15, ['Q'] = 16, ['R'] = 17,
        ['S'] = 18, ['T'] = 19, ['U'] = 20, ['V'] = 21, ['W'] = 22, ['X'] = 23,
        ['Y'] = 24, ['Z'] = 25,
        ['a'] = 26, ['b'] = 27, ['c'] = 28, ['d'] = 29, ['e'] = 30, ['f'] = 31,
        ['g'] = 32, ['h'] = 33, ['i'] = 34, ['j'] = 35, ['k'] = 36, ['l'] = 37,
        ['m'] = 38, ['n'] = 39, ['o'] = 40, ['p'] = 41, ['q'] = 42, ['r'] = 43,
        ['s'] = 44, ['t'] = 45, ['u'] = 46, ['v'] = 47, ['w'] = 48, ['x'] = 49,
        ['y'] = 50, ['z'] = 51,
        ['0'] = 52, ['1'] = 53, ['2'] = 54, ['3'] = 55, ['4'] = 56, ['5'] = 57,
        ['6'] = 58, ['7'] = 59, ['8'] = 60, ['9'] = 61,
        ['+'] = 62, ['/'] = 63
    };
    if (input_len % 4 != 0) return -1;
    size_t decoded_len = input_len / 4 * 3;
    if (input_len > 0 && input[input_len - 1] == '=') decoded_len--;
    if (input_len > 1 && input[input_len - 2] == '=') decoded_len--;
    if (decoded_len > out_cap) return -1;

    size_t j = 0;
    for (size_t i = 0; i < input_len; i += 4) {
        int8_t a = decode_table[(unsigned char)input[i]];
        int8_t b = (i + 1 < input_len) ? decode_table[(unsigned char)input[i + 1]] : -1;
        if (a < 0 || b < 0) return -1;
        uint32_t triple = ((uint32_t)a << 18) | ((uint32_t)b << 12);
        if (i + 2 < input_len && input[i + 2] != '=') {
            int8_t c = decode_table[(unsigned char)input[i + 2]];
            if (c < 0) return -1;
            triple |= ((uint32_t)c << 6);
        }
        if (i + 3 < input_len && input[i + 3] != '=') {
            int8_t d = decode_table[(unsigned char)input[i + 3]];
            if (d < 0) return -1;
            triple |= (uint32_t)d;
        }
        if (j < decoded_len) out[j++] = (uint8_t)(triple >> 16);
        if (j < decoded_len) out[j++] = (uint8_t)(triple >> 8);
        if (j < decoded_len) out[j++] = (uint8_t)triple;
    }
    return (int)decoded_len;
}

// ── Compute Sec-WebSocket-Accept (NET4-4b) ──────────────────
// SHA-1(key + GUID) -> Base64
static const char *WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

static char *taida_net4_compute_ws_accept(const char *key) {
    // Concatenate key + GUID
    size_t key_len = strlen(key);
    size_t guid_len = strlen(WS_GUID);
    size_t total = key_len + guid_len;
    uint8_t *combined = (uint8_t*)TAIDA_MALLOC(total + 1, "net_ws_accept");
    memcpy(combined, key, key_len);
    memcpy(combined + key_len, WS_GUID, guid_len);

    uint8_t digest[20];
    taida_sha1(combined, total, digest);
    free(combined);

    return taida_base64_encode(digest, 20);
}

// ── WebSocket constants ──────────────────────────────────────
#define WS_OPCODE_TEXT   0x1
#define WS_OPCODE_BINARY 0x2
#define WS_OPCODE_CLOSE  0x8
#define WS_OPCODE_PING   0x9
#define WS_OPCODE_PONG   0xA
#define WS_MAX_PAYLOAD   (16ULL * 1024 * 1024)  // 16 MiB

// ── v4 Body streaming helpers ────────────────────────────────

// Read exactly `count` bytes from fd. Returns bytes read, 0 on error/EOF.
// NET5-4a: Routes through TLS when tl_ssl is active.
static size_t taida_net4_recv_exact(int fd, unsigned char *out, size_t count) {
    return taida_tls_recv_exact(fd, out, count);
}

// Read up to `count` bytes from leftover then fd.
// Returns a new Bytes object (caller's ownership), or empty Bytes on EOF.
// NET5-4a: Routes through TLS when tl_ssl is active.
static size_t taida_net4_read_body_bytes(Net4BodyState *bs, int fd, unsigned char *out, size_t count) {
    size_t total = 0;
    // First, drain from leftover.
    while (total < count && bs->leftover_pos < bs->leftover_len) {
        out[total++] = bs->leftover[bs->leftover_pos++];
    }
    // Then read from socket (TLS-aware).
    while (total < count) {
        ssize_t n = taida_tls_recv(fd, out + total, count - total);
        if (n <= 0) {
            if (n < 0 && errno == EINTR) continue;
            break; // EOF or error
        }
        total += (size_t)n;
    }
    return total;
}

// Read a line (up to LF) from leftover then fd.
// Returns line in `out` (null-terminated). Max `cap` bytes including NUL.
// Returns length excluding NUL.
// NET5-4a: Routes through TLS when tl_ssl is active.
static size_t taida_net4_read_line(Net4BodyState *bs, int fd, char *out, size_t cap) {
    size_t pos = 0;
    // From leftover.
    while (pos < cap - 1 && bs->leftover_pos < bs->leftover_len) {
        unsigned char b = bs->leftover[bs->leftover_pos++];
        out[pos++] = (char)b;
        if (b == '\n') { out[pos] = '\0'; return pos; }
    }
    // From socket byte-by-byte (TLS-aware).
    while (pos < cap - 1) {
        unsigned char b;
        ssize_t n = taida_tls_recv(fd, &b, 1);
        if (n <= 0) {
            if (n < 0 && errno == EINTR) continue;
            break;
        }
        out[pos++] = (char)b;
        if (b == '\n') break;
    }
    out[pos] = '\0';
    return pos;
}

// Drain chunked trailers after terminal chunk (NB4-8 parity).
// Returns 0 on success, -1 on protocol error (missing final CRLF).
static int taida_net4_drain_chunked_trailers(Net4BodyState *bs, int fd) {
    char line[4096];
    for (int i = 0; i < 64; i++) {
        size_t len = taida_net4_read_line(bs, fd, line, sizeof(line));
        // NB4-18: EOF (0 raw bytes) != valid empty line ("\r\n").
        if (len == 0) {
            fprintf(stderr, "chunked body error: missing final CRLF after terminal chunk\n");
            return -1;
        }
        // Trim whitespace and check empty.
        size_t start = 0, end = len;
        while (start < end && (line[start] == ' ' || line[start] == '\t' || line[start] == '\r' || line[start] == '\n')) start++;
        while (end > start && (line[end-1] == ' ' || line[end-1] == '\t' || line[end-1] == '\r' || line[end-1] == '\n')) end--;
        if (start == end) return 0; // Empty line = trailers done.
    }
    return 0;
}

// Make Lax[Bytes] empty (parity with Interpreter: hasValue=false).
static taida_val taida_net4_make_lax_bytes_empty(void) {
    return taida_lax_empty(taida_bytes_default_value());
}

// Make Lax[Bytes] with value (parity with Interpreter: hasValue=true).
static taida_val taida_net4_make_lax_bytes_value(const unsigned char *data, size_t len) {
    taida_val bytes = taida_bytes_from_raw(data, (taida_val)len);
    return taida_lax_new(bytes, taida_bytes_default_value());
}

// Validate that req is a body-streaming request pack.
static int taida_net4_is_body_stream_request(taida_val req) {
    if (!taida_is_buchi_pack(req)) return 0;
    taida_val sentinel = taida_pack_get(req, taida_str_hash((taida_val)"__body_stream"));
    if (sentinel == 0) return 0;
    const char *s = (const char*)sentinel;
    size_t slen = 0;
    if (!taida_read_cstr_len_safe(s, 64, &slen)) return 0;
    return (slen == 16 && memcmp(s, "__v4_body_stream", 16) == 0);
}

// Extract __body_token from request pack.
static uint64_t taida_net4_extract_body_token(taida_val req) {
    return (uint64_t)taida_pack_get(req, taida_str_hash((taida_val)"__body_token"));
}

// ── readBodyChunk(req) → Lax[Bytes] ─────────────────────────
taida_val taida_net_read_body_chunk(taida_val req) {
    if (!taida_net4_is_body_stream_request(req)) {
        fprintf(stderr, "readBodyChunk: can only be called in a 2-argument httpServe handler. "
                "In a 1-argument handler, the request body is already fully read. "
                "Use readBody(req) instead.\n");
        exit(1);
    }

    Net4BodyState *bs = tl_net4_body;
    if (!bs) {
        fprintf(stderr, "readBodyChunk: no active body streaming state\n");
        exit(1);
    }

    // NB4-7: Verify request token.
    uint64_t tok = taida_net4_extract_body_token(req);
    if (tok != bs->request_token) {
        fprintf(stderr, "readBodyChunk: request pack does not match the current active request. "
                "The request may be stale or fabricated.\n");
        exit(1);
    }

    Net3WriterState *w = tl_net3_writer;
    if (w && w->state == NET3_STATE_WEBSOCKET) {
        fprintf(stderr, "readBodyChunk: cannot read HTTP body after WebSocket upgrade.\n");
        exit(1);
    }

    int fd = tl_net3_client_fd;

    bs->any_read_started = 1;

    if (bs->fully_read) {
        return taida_net4_make_lax_bytes_empty();
    }

    if (bs->is_chunked) {
        // Chunked TE decode (parity with Interpreter).
        #define NET4_READ_BUF 8192
        char line_buf[4096];
        for (;;) {
            switch (bs->chunked_state) {
                case NET4_CHUNKED_DONE:
                    bs->fully_read = 1;
                    return taida_net4_make_lax_bytes_empty();

                case NET4_CHUNKED_WAIT_SIZE: {
                    size_t llen = taida_net4_read_line(bs, fd, line_buf, sizeof(line_buf));
                    // Trim.
                    size_t s = 0, e = llen;
                    while (s < e && (line_buf[s]==' '||line_buf[s]=='\t'||line_buf[s]=='\r'||line_buf[s]=='\n')) s++;
                    while (e > s && (line_buf[e-1]==' '||line_buf[e-1]=='\t'||line_buf[e-1]=='\r'||line_buf[e-1]=='\n')) e--;
                    if (s == e) continue; // Empty line, try again.
                    // Parse hex chunk-size (strip chunk-extension after ';').
                    char hex_buf[64];
                    size_t hex_len = 0;
                    for (size_t i = s; i < e && line_buf[i] != ';' && hex_len < 63; i++) {
                        if (line_buf[i] != ' ' && line_buf[i] != '\t')
                            hex_buf[hex_len++] = line_buf[i];
                    }
                    hex_buf[hex_len] = '\0';
                    // NB4-18: Strict hex-only parse. Reject partial parse like '1g'.
                    for (size_t vi = 0; vi < hex_len; vi++) {
                        char c = hex_buf[vi];
                        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'))) {
                            fprintf(stderr, "readBodyChunk: invalid chunk-size '%s' in chunked body\n", hex_buf);
                            exit(1);
                        }
                    }
                    if (hex_len == 0) continue; // skip empty, retry
                    unsigned long chunk_size = strtoul(hex_buf, NULL, 16);
                    if (chunk_size == 0) {
                        bs->chunked_state = NET4_CHUNKED_DONE;
                        bs->fully_read = 1;
                        if (taida_net4_drain_chunked_trailers(bs, fd) < 0) {
                            bs->fully_read = 0;
                            fprintf(stderr, "readBodyChunk: chunked body protocol error\n");
                            exit(1);
                        }
                        return taida_net4_make_lax_bytes_empty();
                    }
                    bs->chunked_state = NET4_CHUNKED_READ_DATA;
                    bs->chunked_remaining = (size_t)chunk_size;
                    break;
                }

                case NET4_CHUNKED_READ_DATA: {
                    if (bs->chunked_remaining == 0) {
                        bs->chunked_state = NET4_CHUNKED_WAIT_TRAILER;
                        continue;
                    }
                    size_t to_read = bs->chunked_remaining;
                    if (to_read > NET4_READ_BUF) to_read = NET4_READ_BUF;
                    unsigned char tmp[NET4_READ_BUF];
                    size_t got = taida_net4_read_body_bytes(bs, fd, tmp, to_read);
                    // NB4-18: short read (EOF) in chunked data is a protocol error.
                    if (got == 0) {
                        fprintf(stderr, "readBodyChunk: truncated chunked body — expected %zu more chunk-data bytes but got EOF\n",
                                bs->chunked_remaining);
                        exit(1);
                    }
                    bs->chunked_remaining -= got;
                    bs->bytes_consumed += (int64_t)got;
                    return taida_net4_make_lax_bytes_value(tmp, got);
                }

                case NET4_CHUNKED_WAIT_TRAILER: {
                    // NB4-18: Read CRLF after chunk data and validate.
                    {
                        size_t tl_len = taida_net4_read_line(bs, fd, line_buf, sizeof(line_buf));
                        if (tl_len == 0) {
                            fprintf(stderr, "readBodyChunk: missing CRLF after chunk data (unexpected EOF)\n");
                            exit(1);
                        }
                        // Trim and check empty.
                        size_t ts = 0, te = tl_len;
                        while (ts < te && (line_buf[ts]==' '||line_buf[ts]=='\t'||line_buf[ts]=='\r'||line_buf[ts]=='\n')) ts++;
                        while (te > ts && (line_buf[te-1]==' '||line_buf[te-1]=='\t'||line_buf[te-1]=='\r'||line_buf[te-1]=='\n')) te--;
                        if (ts != te) {
                            line_buf[tl_len < sizeof(line_buf)-1 ? tl_len : sizeof(line_buf)-1] = '\0';
                            fprintf(stderr, "readBodyChunk: malformed chunk trailer — expected CRLF after chunk data, got \"%s\"\n", line_buf);
                            exit(1);
                        }
                    }
                    bs->chunked_state = NET4_CHUNKED_WAIT_SIZE;
                    break;
                }
            }
        }
        #undef NET4_READ_BUF
    } else {
        // Content-Length path.
        int64_t remaining = bs->content_length - bs->bytes_consumed;
        if (remaining <= 0) {
            bs->fully_read = 1;
            return taida_net4_make_lax_bytes_empty();
        }
        size_t to_read = (size_t)remaining;
        if (to_read > 8192) to_read = 8192;
        unsigned char tmp[8192];
        size_t got = taida_net4_read_body_bytes(bs, fd, tmp, to_read);
        if (got == 0) {
            // NB4-18: EOF before Content-Length exhausted is a protocol error.
            fprintf(stderr, "readBodyChunk: truncated body — expected %" PRId64
                    " bytes (Content-Length) but got EOF after %" PRId64 " bytes\n",
                    bs->content_length, bs->bytes_consumed);
            exit(1);
        }
        bs->bytes_consumed += (int64_t)got;
        if (bs->bytes_consumed >= bs->content_length) {
            bs->fully_read = 1;
        }
        return taida_net4_make_lax_bytes_value(tmp, got);
    }
}

// ── readBodyAll(req) → Bytes ─────────────────────────────────
// The only aggregate path permitted by v4 contract.
taida_val taida_net_read_body_all(taida_val req) {
    if (!taida_net4_is_body_stream_request(req)) {
        fprintf(stderr, "readBodyAll: can only be called in a 2-argument httpServe handler. "
                "In a 1-argument handler, the request body is already fully read. "
                "Use readBody(req) instead.\n");
        exit(1);
    }

    Net4BodyState *bs = tl_net4_body;
    if (!bs) {
        fprintf(stderr, "readBodyAll: no active body streaming state\n");
        exit(1);
    }

    // NB4-7: Verify request token.
    uint64_t tok = taida_net4_extract_body_token(req);
    if (tok != bs->request_token) {
        fprintf(stderr, "readBodyAll: request pack does not match the current active request.\n");
        exit(1);
    }

    Net3WriterState *w = tl_net3_writer;
    if (w && w->state == NET3_STATE_WEBSOCKET) {
        fprintf(stderr, "readBodyAll: cannot read HTTP body after WebSocket upgrade.\n");
        exit(1);
    }

    int fd = tl_net3_client_fd;

    bs->any_read_started = 1;

    if (bs->fully_read) {
        return taida_bytes_new_filled(0, 0);
    }

    // Aggregate all remaining body bytes (this is the only permitted aggregate path).
    size_t all_cap = 4096;
    size_t all_len = 0;
    unsigned char *all_buf = (unsigned char*)TAIDA_MALLOC(all_cap, "net_readBodyAll");

    if (bs->is_chunked) {
        char line_buf[4096];
        for (;;) {
            switch (bs->chunked_state) {
                case NET4_CHUNKED_DONE:
                    bs->fully_read = 1;
                    goto all_done;

                case NET4_CHUNKED_WAIT_SIZE: {
                    size_t llen = taida_net4_read_line(bs, fd, line_buf, sizeof(line_buf));
                    size_t s = 0, e = llen;
                    while (s < e && (line_buf[s]==' '||line_buf[s]=='\t'||line_buf[s]=='\r'||line_buf[s]=='\n')) s++;
                    while (e > s && (line_buf[e-1]==' '||line_buf[e-1]=='\t'||line_buf[e-1]=='\r'||line_buf[e-1]=='\n')) e--;
                    if (s == e) continue;
                    char hex_buf[64];
                    size_t hex_len = 0;
                    for (size_t i = s; i < e && line_buf[i] != ';' && hex_len < 63; i++) {
                        if (line_buf[i] != ' ' && line_buf[i] != '\t')
                            hex_buf[hex_len++] = line_buf[i];
                    }
                    hex_buf[hex_len] = '\0';
                    // NB4-18: Strict hex-only parse. Reject partial parse like '1g'.
                    for (size_t vi = 0; vi < hex_len; vi++) {
                        char c = hex_buf[vi];
                        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'))) {
                            fprintf(stderr, "readBodyChunk: invalid chunk-size '%s' in chunked body\n", hex_buf);
                            exit(1);
                        }
                    }
                    if (hex_len == 0) continue; // skip empty, retry
                    unsigned long chunk_size = strtoul(hex_buf, NULL, 16);
                    if (chunk_size == 0) {
                        bs->chunked_state = NET4_CHUNKED_DONE;
                        bs->fully_read = 1;
                        if (taida_net4_drain_chunked_trailers(bs, fd) < 0) {
                            bs->fully_read = 0;
                            fprintf(stderr, "readBodyAll: chunked body protocol error\n");
                            exit(1);
                        }
                        goto all_done;
                    }
                    bs->chunked_state = NET4_CHUNKED_READ_DATA;
                    bs->chunked_remaining = (size_t)chunk_size;
                    break;
                }

                case NET4_CHUNKED_READ_DATA: {
                    if (bs->chunked_remaining == 0) {
                        bs->chunked_state = NET4_CHUNKED_WAIT_TRAILER;
                        continue;
                    }
                    // Ensure capacity.
                    while (all_len + bs->chunked_remaining > all_cap) {
                        all_cap *= 2;
                        TAIDA_REALLOC(all_buf, all_cap, "net_readBodyAll_grow");
                    }
                    size_t got = taida_net4_read_body_bytes(bs, fd, all_buf + all_len, bs->chunked_remaining);
                    // NB4-18: short read (EOF) in chunked data is a protocol error.
                    if (got == 0) {
                        fprintf(stderr, "readBodyAll: truncated chunked body — expected %zu more chunk-data bytes but got EOF\n",
                                bs->chunked_remaining);
                        free(all_buf);
                        exit(1);
                    }
                    all_len += got;
                    size_t new_rem = bs->chunked_remaining - got;
                    bs->chunked_remaining = new_rem;
                    break;
                }

                case NET4_CHUNKED_WAIT_TRAILER: {
                    // NB4-18: Read CRLF after chunk data and validate.
                    {
                        size_t tl_len2 = taida_net4_read_line(bs, fd, line_buf, sizeof(line_buf));
                        if (tl_len2 == 0) {
                            fprintf(stderr, "readBodyAll: missing CRLF after chunk data (unexpected EOF)\n");
                            free(all_buf);
                            exit(1);
                        }
                        size_t ts2 = 0, te2 = tl_len2;
                        while (ts2 < te2 && (line_buf[ts2]==' '||line_buf[ts2]=='\t'||line_buf[ts2]=='\r'||line_buf[ts2]=='\n')) ts2++;
                        while (te2 > ts2 && (line_buf[te2-1]==' '||line_buf[te2-1]=='\t'||line_buf[te2-1]=='\r'||line_buf[te2-1]=='\n')) te2--;
                        if (ts2 != te2) {
                            line_buf[tl_len2 < sizeof(line_buf)-1 ? tl_len2 : sizeof(line_buf)-1] = '\0';
                            fprintf(stderr, "readBodyAll: malformed chunk trailer — expected CRLF after chunk data, got \"%s\"\n", line_buf);
                            free(all_buf);
                            exit(1);
                        }
                    }
                    bs->chunked_state = NET4_CHUNKED_WAIT_SIZE;
                    break;
                }
            }
        }
    } else {
        // Content-Length path.
        int64_t remaining = bs->content_length - bs->bytes_consumed;
        if (remaining > 0) {
            size_t to_read = (size_t)remaining;
            if (to_read > all_cap) {
                all_cap = to_read;
                TAIDA_REALLOC(all_buf, all_cap, "net_readBodyAll_cl");
            }
            size_t got = taida_net4_read_body_bytes(bs, fd, all_buf, to_read);
            // NB4-18: EOF before Content-Length exhausted is a protocol error.
            if (got == 0 && to_read > 0) {
                fprintf(stderr, "readBodyAll: truncated body — expected %" PRId64
                        " bytes (Content-Length) but got EOF after %" PRId64 " bytes\n",
                        bs->content_length, bs->bytes_consumed);
                free(all_buf);
                exit(1);
            }
            all_len = got;
            bs->bytes_consumed += (int64_t)got;
        }
        bs->fully_read = 1;
    }

all_done:;
    // D29B-003 (Track-β, 2026-04-27): kept on legacy form pending the
    // polymorphic Bytes dispatcher follow-up (see readBody producer above
    // for the rationale). The writev hot path supports both legacy and
    // CONTIG inputs; opt-in CONTIG construction is via taida_bytes_contig_new.
    taida_val result = taida_bytes_from_raw(all_buf, (taida_val)all_len);
    free(all_buf);
    return result;
}

// ── WebSocket frame write (NET4-4c) ─────────────────────────
// Server->client: FIN=1, MASK=0. Header on stack, payload via writev.
static int taida_net4_write_ws_frame(int fd, uint8_t opcode, const unsigned char *payload, size_t payload_len) {
    unsigned char header[10];
    int header_len;
    header[0] = 0x80 | opcode; // FIN=1
    if (payload_len < 126) {
        header[1] = (uint8_t)payload_len;
        header_len = 2;
    } else if (payload_len <= 65535) {
        header[1] = 126;
        header[2] = (uint8_t)(payload_len >> 8);
        header[3] = (uint8_t)(payload_len & 0xFF);
        header_len = 4;
    } else {
        header[1] = 127;
        uint64_t len64 = (uint64_t)payload_len;
        header[2] = (uint8_t)(len64 >> 56);
        header[3] = (uint8_t)(len64 >> 48);
        header[4] = (uint8_t)(len64 >> 40);
        header[5] = (uint8_t)(len64 >> 32);
        header[6] = (uint8_t)(len64 >> 24);
        header[7] = (uint8_t)(len64 >> 16);
        header[8] = (uint8_t)(len64 >> 8);
        header[9] = (uint8_t)(len64);
        header_len = 10;
    }
    // Vectored write: header + payload (no aggregate buffer).
    struct iovec iov[2];
    iov[0].iov_base = header;
    iov[0].iov_len = (size_t)header_len;
    iov[1].iov_base = (void*)payload;
    iov[1].iov_len = payload_len;
    return taida_net_writev_all(fd, iov, payload_len > 0 ? 2 : 1);
}

// ── WebSocket frame read (NET4-4c) ──────────────────────────
// Frame types returned by read_ws_frame.
#define WS_FRAME_TEXT     1
#define WS_FRAME_BINARY   2
#define WS_FRAME_PING     3
#define WS_FRAME_PONG     4
#define WS_FRAME_CLOSE    5
#define WS_FRAME_ERROR    6

typedef struct {
    int type;                // WS_FRAME_*
    unsigned char *payload;  // heap-allocated payload (caller must free)
    size_t payload_len;
    uint8_t opcode;
} WsFrameResult;

static WsFrameResult taida_net4_read_ws_frame(int fd) {
    WsFrameResult result = { WS_FRAME_ERROR, NULL, 0, 0 };
    unsigned char hdr[2];
    if (taida_net4_recv_exact(fd, hdr, 2) != 2) {
        return result;
    }
    uint8_t byte0 = hdr[0], byte1 = hdr[1];
    int fin = (byte0 & 0x80) != 0;
    uint8_t rsv = byte0 & 0x70;
    uint8_t opcode = byte0 & 0x0F;
    int masked = (byte1 & 0x80) != 0;
    uint64_t payload_len7 = byte1 & 0x7F;

    // RSV must be 0.
    if (rsv != 0) { result.type = WS_FRAME_ERROR; return result; }

    // Fragmented frames not supported.
    if (!fin) { result.type = WS_FRAME_ERROR; return result; }

    // Continuation frame without fragmentation is error.
    if (opcode == 0x0) { result.type = WS_FRAME_ERROR; return result; }

    // NB4-11: Client-to-server frames MUST be masked (RFC 6455 Section 5.1).
    if (!masked) { result.type = WS_FRAME_ERROR; return result; }

    // Determine actual payload length.
    uint64_t payload_len;
    if (payload_len7 < 126) {
        payload_len = payload_len7;
    } else if (payload_len7 == 126) {
        unsigned char ext[2];
        if (taida_net4_recv_exact(fd, ext, 2) != 2) return result;
        payload_len = ((uint64_t)ext[0] << 8) | ext[1];
    } else { // 127
        unsigned char ext[8];
        if (taida_net4_recv_exact(fd, ext, 8) != 8) return result;
        payload_len = 0;
        for (int i = 0; i < 8; i++) payload_len = (payload_len << 8) | ext[i];
        if (payload_len >> 63) { result.type = WS_FRAME_ERROR; return result; }
    }

    // Oversized payload check.
    if (payload_len > WS_MAX_PAYLOAD) { result.type = WS_FRAME_ERROR; return result; }

    // Read masking key if masked.
    uint8_t mask_key[4] = {0};
    if (masked) {
        if (taida_net4_recv_exact(fd, mask_key, 4) != 4) return result;
    }

    // NB6-9: Read payload using stack buffer for small frames (<=4KB) to avoid
    // per-frame malloc/free overhead for high-frequency small WebSocket messages.
    // Heap fallback for larger payloads.
    unsigned char stack_payload[4096];
    unsigned char *payload = NULL;
    int payload_on_heap = 0;
    if (payload_len > 0) {
        if ((size_t)payload_len <= sizeof(stack_payload)) {
            payload = stack_payload;
        } else {
            payload = (unsigned char*)TAIDA_MALLOC((size_t)payload_len, "net_ws_frame_payload");
            payload_on_heap = 1;
        }
        if (taida_net4_recv_exact(fd, payload, (size_t)payload_len) != (size_t)payload_len) {
            if (payload_on_heap) free(payload);
            return result;
        }
        // NB6-6: Unmask in-place using word-at-a-time XOR.
        // Process 4 bytes at a time to eliminate modulo per byte.
        if (masked) {
            uint32_t mask_word;
            memcpy(&mask_word, mask_key, 4);
            size_t plen = (size_t)payload_len;
            size_t i = 0;
            // Word-at-a-time loop.
            for (; i + 4 <= plen; i += 4) {
                uint32_t word;
                memcpy(&word, payload + i, 4);
                word ^= mask_word;
                memcpy(payload + i, &word, 4);
            }
            // Handle remaining 1-3 bytes.
            for (; i < plen; i++) {
                payload[i] ^= mask_key[i & 3];
            }
        }
    }

    // NB6-9: If payload was on stack, copy to heap for caller to free.
    if (payload && !payload_on_heap) {
        unsigned char *heap_copy = (unsigned char*)TAIDA_MALLOC((size_t)payload_len, "net_ws_frame_payload");
        memcpy(heap_copy, payload, (size_t)payload_len);
        payload = heap_copy;
    }
    result.payload = payload;
    result.payload_len = (size_t)payload_len;
    result.opcode = opcode;

    switch (opcode) {
        case WS_OPCODE_TEXT:   result.type = WS_FRAME_TEXT; break;
        case WS_OPCODE_BINARY: result.type = WS_FRAME_BINARY; break;
        case WS_OPCODE_CLOSE:  result.type = WS_FRAME_CLOSE; break;
        case WS_OPCODE_PING:   result.type = WS_FRAME_PING; break;
        case WS_OPCODE_PONG:   result.type = WS_FRAME_PONG; break;
        default:               result.type = WS_FRAME_ERROR; break;
    }
    return result;
}

// ── NB4-10: Validate WsConn token — sentinel + connection-scoped token ──
static int taida_net4_validate_ws_token(taida_val ws) {
    if (!taida_is_buchi_pack(ws)) return 0;
    // Check sentinel.
    taida_val id_val = taida_pack_get(ws, taida_str_hash((taida_val)"__ws_id"));
    if (id_val == 0) return 0;
    const char *id_str = (const char*)id_val;
    size_t id_len = 0;
    if (!taida_read_cstr_len_safe(id_str, 64, &id_len)) return 0;
    if (id_len != 19 || memcmp(id_str, "__v4_websocket_conn", 19) != 0) return 0;
    // Verify connection-scoped token matches active ws_token.
    Net4BodyState *bs = tl_net4_body;
    if (!bs || bs->ws_token == 0) return 0;
    taida_val tok_val = taida_pack_get(ws, taida_str_hash((taida_val)"__ws_token"));
    if ((uint64_t)tok_val != bs->ws_token) return 0;
    return 1;
}

// Make Lax[@(ws: WsConn)] with value.
static taida_val taida_net4_make_lax_ws_value(taida_val ws_pack) {
    taida_val inner = taida_pack_new(1);
    taida_pack_set_hash(inner, 0, taida_str_hash((taida_val)"ws"));
    taida_pack_set(inner, 0, ws_pack);
    taida_pack_set_tag(inner, 0, TAIDA_TAG_PACK);
    taida_retain(ws_pack);
    return taida_lax_new(inner, taida_pack_new(0));
}

// Make Lax empty for failed wsUpgrade.
static taida_val taida_net4_make_lax_ws_empty(void) {
    return taida_lax_empty(taida_pack_new(0));
}

// Make Lax[@(type, data)] for wsReceive data frame.
static taida_val taida_net4_make_lax_ws_frame_value(const char *type_str, taida_val data_val) {
    taida_val inner = taida_pack_new(2);
    taida_pack_set_hash(inner, 0, taida_str_hash((taida_val)"type"));
    taida_pack_set(inner, 0, (taida_val)taida_str_new_copy(type_str));
    taida_pack_set_tag(inner, 0, TAIDA_TAG_STR);
    taida_pack_set_hash(inner, 1, taida_str_hash((taida_val)"data"));
    taida_pack_set(inner, 1, data_val);
    // Tag the data field appropriately.
    if (TAIDA_IS_BYTES(data_val)) {
        taida_pack_set_tag(inner, 1, TAIDA_TAG_PACK); // Bytes is ptr
    } else {
        taida_pack_set_tag(inner, 1, TAIDA_TAG_STR);
    }
    return taida_lax_new(inner, taida_pack_new(0));
}

// Make Lax empty for wsReceive close / end of stream.
static taida_val taida_net4_make_lax_ws_frame_empty(void) {
    return taida_lax_empty(taida_pack_new(0));
}

// ── Helper: extract header value from request pack (case-insensitive) ──
static int taida_net4_get_header_value(taida_val req, const unsigned char *raw, size_t raw_len,
                                        const char *target_name, char *out, size_t out_cap) {
    taida_val headers = taida_pack_get(req, taida_str_hash((taida_val)"headers"));
    if (!TAIDA_IS_LIST(headers)) return 0;
    taida_val *hdr_list = (taida_val*)headers;
    taida_val hdr_count = hdr_list[2];
    size_t target_len = strlen(target_name);

    for (taida_val i = 0; i < hdr_count; i++) {
        taida_val header = hdr_list[4 + i];
        if (!taida_is_buchi_pack(header)) continue;

        taida_val name_span = taida_pack_get(header, taida_str_hash((taida_val)"name"));
        if (!taida_is_buchi_pack(name_span)) continue;
        taida_val n_start = taida_pack_get(name_span, taida_str_hash((taida_val)"start"));
        taida_val n_len = taida_pack_get(name_span, taida_str_hash((taida_val)"len"));
        if (n_start < 0 || n_len <= 0 || (size_t)(n_start + n_len) > raw_len) continue;
        if ((size_t)n_len != target_len) continue;

        int match = 1;
        for (size_t j = 0; j < target_len; j++) {
            char c = (char)raw[n_start + j];
            if (c >= 'A' && c <= 'Z') c += 32;
            char t = target_name[j];
            if (t >= 'A' && t <= 'Z') t += 32;
            if (c != t) { match = 0; break; }
        }
        if (!match) continue;

        taida_val val_span = taida_pack_get(header, taida_str_hash((taida_val)"value"));
        if (!taida_is_buchi_pack(val_span)) continue;
        taida_val v_start = taida_pack_get(val_span, taida_str_hash((taida_val)"start"));
        taida_val v_len = taida_pack_get(val_span, taida_str_hash((taida_val)"len"));
        if (v_start < 0 || v_len <= 0 || (size_t)(v_start + v_len) > raw_len) continue;

        size_t copy_len = (size_t)v_len;
        if (copy_len >= out_cap) copy_len = out_cap - 1;
        memcpy(out, raw + v_start, copy_len);
        out[copy_len] = '\0';
        return 1;
    }
    return 0;
}

// ── Helper: extract method string from request ──
static int taida_net4_get_method(taida_val req, const unsigned char *raw, size_t raw_len, char *out, size_t out_cap) {
    taida_val method_span = taida_pack_get(req, taida_str_hash((taida_val)"method"));
    if (!taida_is_buchi_pack(method_span)) return 0;
    taida_val m_start = taida_pack_get(method_span, taida_str_hash((taida_val)"start"));
    taida_val m_len = taida_pack_get(method_span, taida_str_hash((taida_val)"len"));
    if (m_start < 0 || m_len <= 0 || (size_t)(m_start + m_len) > raw_len) return 0;
    size_t copy_len = (size_t)m_len;
    if (copy_len >= out_cap) copy_len = out_cap - 1;
    memcpy(out, raw + m_start, copy_len);
    out[copy_len] = '\0';
    return 1;
}

// Case-insensitive string compare.
static int taida_net4_strcasecmp(const char *a, const char *b) {
    while (*a && *b) {
        char ca = *a, cb = *b;
        if (ca >= 'A' && ca <= 'Z') ca += 32;
        if (cb >= 'A' && cb <= 'Z') cb += 32;
        if (ca != cb) return ca - cb;
        a++; b++;
    }
    return (unsigned char)*a - (unsigned char)*b;
}

// Check if a comma-separated header value contains a token (case-insensitive).
static int taida_net4_header_contains_token(const char *value, const char *token) {
    size_t token_len = strlen(token);
    const char *p = value;
    while (*p) {
        // Skip leading whitespace and commas.
        while (*p == ' ' || *p == '\t' || *p == ',') p++;
        if (!*p) break;
        const char *start = p;
        while (*p && *p != ',') p++;
        // Trim trailing whitespace.
        const char *end = p;
        while (end > start && (end[-1] == ' ' || end[-1] == '\t')) end--;
        size_t tlen = (size_t)(end - start);
        if (tlen == token_len) {
            int match = 1;
            for (size_t i = 0; i < tlen; i++) {
                char ca = start[i], cb = token[i];
                if (ca >= 'A' && ca <= 'Z') ca += 32;
                if (cb >= 'A' && cb <= 'Z') cb += 32;
                if (ca != cb) { match = 0; break; }
            }
            if (match) return 1;
        }
    }
    return 0;
}

// ── wsUpgrade(req, writer) → Lax[@(ws: WsConn)] (NET4-4b) ──
taida_val taida_net_ws_upgrade(taida_val req, taida_val writer) {
    // Must be inside 2-arg handler.
    Net3WriterState *w = tl_net3_writer;
    if (!w) {
        fprintf(stderr, "wsUpgrade: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }

    // Validate writer token.
    taida_net3_validate_writer(writer, "wsUpgrade");

    // NB4-10: Verify request token matches the active body state.
    {
        Net4BodyState *bs_check = tl_net4_body;
        if (bs_check) {
            uint64_t tok = taida_net4_extract_body_token(req);
            if (tok != bs_check->request_token) {
                fprintf(stderr, "wsUpgrade: request pack does not match the current active request. "
                        "The request may be stale or fabricated.\n");
                exit(1);
            }
        }
    }

    // State check: only valid in Idle state.
    switch (w->state) {
        case NET3_STATE_IDLE: break;
        case NET3_STATE_HEAD_PREPARED:
        case NET3_STATE_STREAMING:
            fprintf(stderr, "wsUpgrade: cannot upgrade after HTTP response has started. "
                    "wsUpgrade must be called before startResponse/writeChunk.\n");
            exit(1);
        case NET3_STATE_ENDED:
            fprintf(stderr, "wsUpgrade: cannot upgrade after HTTP response has ended.\n");
            exit(1);
        case NET3_STATE_WEBSOCKET:
            fprintf(stderr, "wsUpgrade: WebSocket upgrade already completed.\n");
            exit(1);
    }

    if (!taida_is_buchi_pack(req)) {
        return taida_net4_make_lax_ws_empty();
    }

    // Extract raw bytes for header value extraction.
    taida_val raw_val = taida_pack_get(req, taida_str_hash((taida_val)"raw"));
    if (!TAIDA_IS_BYTES(raw_val)) {
        return taida_net4_make_lax_ws_empty();
    }
    taida_val *raw_arr = (taida_val*)raw_val;
    taida_val raw_len = raw_arr[1];
    // Materialize raw bytes for C string comparison.
    unsigned char *raw = (unsigned char*)TAIDA_MALLOC((size_t)raw_len + 1, "net_ws_raw");
    for (taida_val i = 0; i < raw_len; i++) raw[i] = (unsigned char)raw_arr[2 + i];
    raw[raw_len] = 0;

    // Validate: must be GET.
    char method[16];
    if (!taida_net4_get_method(req, raw, (size_t)raw_len, method, sizeof(method)) ||
        taida_net4_strcasecmp(method, "GET") != 0) {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }

    // Check: no body (Content-Length must be 0 or absent, not chunked).
    taida_val cl = taida_pack_get(req, taida_str_hash((taida_val)"contentLength"));
    taida_val chunked_val = taida_pack_get(req, taida_str_hash((taida_val)"chunked"));
    if (cl > 0 || chunked_val != 0) {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }

    // Validate: Upgrade: websocket
    char hdr_buf[256];
    if (!taida_net4_get_header_value(req, raw, (size_t)raw_len, "upgrade", hdr_buf, sizeof(hdr_buf)) ||
        taida_net4_strcasecmp(hdr_buf, "websocket") != 0) {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }

    // Validate: Connection contains "Upgrade"
    if (!taida_net4_get_header_value(req, raw, (size_t)raw_len, "connection", hdr_buf, sizeof(hdr_buf)) ||
        !taida_net4_header_contains_token(hdr_buf, "Upgrade")) {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }

    // Validate: Sec-WebSocket-Version: 13
    if (!taida_net4_get_header_value(req, raw, (size_t)raw_len, "sec-websocket-version", hdr_buf, sizeof(hdr_buf))) {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }
    // Trim whitespace.
    {
        char *p = hdr_buf;
        while (*p == ' ' || *p == '\t') p++;
        if (strcmp(p, "13") != 0) {
            free(raw);
            return taida_net4_make_lax_ws_empty();
        }
    }

    // Validate: Sec-WebSocket-Key (must be present and non-empty).
    char ws_key[256];
    if (!taida_net4_get_header_value(req, raw, (size_t)raw_len, "sec-websocket-key", ws_key, sizeof(ws_key))) {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }
    // Trim key.
    {
        size_t ks = 0, ke = strlen(ws_key);
        while (ks < ke && (ws_key[ks] == ' ' || ws_key[ks] == '\t')) ks++;
        while (ke > ks && (ws_key[ke-1] == ' ' || ws_key[ke-1] == '\t')) ke--;
        if (ks > 0) memmove(ws_key, ws_key + ks, ke - ks);
        ws_key[ke - ks] = '\0';
    }
    if (ws_key[0] == '\0') {
        free(raw);
        return taida_net4_make_lax_ws_empty();
    }
    // NB4-11: RFC 6455: key must be 24 chars and decode to exactly 16 bytes.
    {
        size_t key_len = strlen(ws_key);
        if (key_len != 24) {
            free(raw);
            return taida_net4_make_lax_ws_empty();
        }
        uint8_t decoded[18]; // 16 bytes + margin
        int dec_len = taida_base64_decode(ws_key, key_len, decoded, sizeof(decoded));
        if (dec_len != 16) {
            free(raw);
            return taida_net4_make_lax_ws_empty();
        }
    }

    free(raw);

    // All validations passed. Compute accept and send 101 response.
    char *accept = taida_net4_compute_ws_accept(ws_key);

    int fd = tl_net3_client_fd;
    char response[512];
    int rlen = snprintf(response, sizeof(response),
        "HTTP/1.1 101 Switching Protocols\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        "Sec-WebSocket-Accept: %s\r\n"
        "\r\n", accept);
    free(accept);

    if (rlen < 0 || (size_t)rlen >= sizeof(response)) {
        return taida_net4_make_lax_ws_empty();
    }
    taida_net_send_all(fd, response, (size_t)rlen);

    // Transition to WebSocket state.
    w->state = NET3_STATE_WEBSOCKET;

    // Mark body state and set ws token.
    Net4BodyState *bs = tl_net4_body;
    uint64_t ws_tok = taida_net4_alloc_ws_token();
    if (bs) {
        bs->ws_closed = 0;
        bs->ws_token = ws_tok;
    }

    // Create WsConn BuchiPack with identity token (NB4-10).
    taida_val ws_pack = taida_pack_new(2);
    taida_pack_set_hash(ws_pack, 0, taida_str_hash((taida_val)"__ws_id"));
    taida_pack_set(ws_pack, 0, (taida_val)"__v4_websocket_conn");
    taida_pack_set_tag(ws_pack, 0, TAIDA_TAG_STR);
    taida_pack_set_hash(ws_pack, 1, taida_str_hash((taida_val)"__ws_token"));
    taida_pack_set(ws_pack, 1, (taida_val)ws_tok);
    taida_pack_set_tag(ws_pack, 1, TAIDA_TAG_INT);

    return taida_net4_make_lax_ws_value(ws_pack);
}

// ── wsSend(ws, data) → Unit (NET4-4d) ───────────────────────
taida_val taida_net_ws_send(taida_val ws, taida_val data) {
    Net3WriterState *w = tl_net3_writer;
    if (!w) {
        fprintf(stderr, "wsSend: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }

    if (!taida_net4_validate_ws_token(ws)) {
        fprintf(stderr, "wsSend: first argument must be the WebSocket connection from wsUpgrade\n");
        exit(1);
    }

    if (w->state != NET3_STATE_WEBSOCKET) {
        fprintf(stderr, "wsSend: not in WebSocket state. Call wsUpgrade first.\n");
        exit(1);
    }

    Net4BodyState *bs = tl_net4_body;
    if (bs && bs->ws_closed) {
        fprintf(stderr, "wsSend: WebSocket connection is already closed.\n");
        exit(1);
    }

    int fd = tl_net3_client_fd;

    // Determine opcode and payload.
    uint8_t opcode;
    const unsigned char *payload;
    size_t payload_len;
    unsigned char *temp_buf = NULL;

    if (TAIDA_IS_BYTES(data)) {
        opcode = WS_OPCODE_BINARY;
        taida_val *bytes = (taida_val*)data;
        taida_val blen = bytes[1];
        payload_len = (size_t)blen;
        temp_buf = (unsigned char*)TAIDA_MALLOC(payload_len + 1, "net_ws_send_bytes");
        for (taida_val i = 0; i < blen; i++) temp_buf[i] = (unsigned char)bytes[2 + i];
        payload = temp_buf;
    } else {
        // Assume Str -> text frame.
        opcode = WS_OPCODE_TEXT;
        const char *s = (const char*)data;
        size_t slen = 0;
        if (!taida_read_cstr_len_safe(s, 64 * 1024 * 1024, &slen)) {
            fprintf(stderr, "wsSend: data must be Str (text frame) or Bytes (binary frame)\n");
            exit(1);
        }
        payload = (const unsigned char*)s;
        payload_len = slen;
    }

    taida_net4_write_ws_frame(fd, opcode, payload, payload_len);
    if (temp_buf) free(temp_buf);

    return 0; // Unit
}

// ── wsReceive(ws) → Lax[@(type, data)] (NET4-4d) ────────────
taida_val taida_net_ws_receive(taida_val ws) {
    Net3WriterState *w = tl_net3_writer;
    if (!w) {
        fprintf(stderr, "wsReceive: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }

    if (!taida_net4_validate_ws_token(ws)) {
        fprintf(stderr, "wsReceive: first argument must be the WebSocket connection from wsUpgrade\n");
        exit(1);
    }

    if (w->state != NET3_STATE_WEBSOCKET) {
        fprintf(stderr, "wsReceive: not in WebSocket state. Call wsUpgrade first.\n");
        exit(1);
    }

    Net4BodyState *bs = tl_net4_body;
    if (bs && bs->ws_closed) {
        return taida_net4_make_lax_ws_frame_empty();
    }

    int fd = tl_net3_client_fd;

    // Loop to handle ping/pong transparently.
    for (;;) {
        WsFrameResult frame = taida_net4_read_ws_frame(fd);

        switch (frame.type) {
            case WS_FRAME_TEXT: {
                // Text frame: return data as Str (parity with Interpreter).
                char *text = NULL;
                if (frame.payload_len > 0) {
                    text = (char*)TAIDA_MALLOC(frame.payload_len + 1, "net_ws_text");
                    memcpy(text, frame.payload, frame.payload_len);
                    text[frame.payload_len] = '\0';
                } else {
                    text = taida_str_new_copy("");
                }
                free(frame.payload);
                taida_val data_val = (taida_val)text;
                return taida_net4_make_lax_ws_frame_value("text", data_val);
            }

            case WS_FRAME_BINARY: {
                taida_val bytes = taida_bytes_from_raw(frame.payload, (taida_val)frame.payload_len);
                free(frame.payload);
                return taida_net4_make_lax_ws_frame_value("binary", bytes);
            }

            case WS_FRAME_PING: {
                // Auto pong: send pong with same payload.
                taida_net4_write_ws_frame(fd, WS_OPCODE_PONG,
                    frame.payload ? frame.payload : (unsigned char*)"",
                    frame.payload_len);
                if (frame.payload) free(frame.payload);
                continue; // Next frame.
            }

            case WS_FRAME_PONG: {
                // Unsolicited pong: ignore.
                if (frame.payload) free(frame.payload);
                continue;
            }

            case WS_FRAME_CLOSE: {
                // v5 close code extraction (NET5-0d).
                if (frame.payload_len == 0) {
                    // No status code: reply with empty close payload.
                    if (bs && !bs->ws_closed) {
                        taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, (unsigned char*)"", 0);
                    }
                    if (bs) {
                        bs->ws_closed = 1;
                        bs->ws_close_code = 1005; // No Status Rcvd
                    }
                    if (frame.payload) free(frame.payload);
                    return taida_net4_make_lax_ws_frame_empty();
                } else if (frame.payload_len == 1) {
                    // 1-byte close payload is malformed.
                    unsigned char close_1002[2] = { 0x03, 0xEA };
                    taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, close_1002, 2);
                    if (bs) bs->ws_closed = 1;
                    if (frame.payload) free(frame.payload);
                    fprintf(stderr, "wsReceive: protocol error: malformed close frame (1-byte payload)\n");
                    exit(1);
                } else {
                    // 2+ bytes: first 2 bytes are the close code (big-endian).
                    uint16_t code = ((uint16_t)frame.payload[0] << 8) | (uint16_t)frame.payload[1];
                    // Validate close code (RFC 6455 Section 7.4).
                    // 1000-1003: standard, 1007-1014: IANA-registered,
                    // 3000-4999: reserved for libraries/apps/private use.
                    int valid_code = (code >= 1000 && code <= 1003) ||
                                     (code >= 1007 && code <= 1014) ||
                                     (code >= 3000 && code <= 4999);
                    if (!valid_code) {
                        unsigned char close_1002[2] = { 0x03, 0xEA };
                        taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, close_1002, 2);
                        if (bs) bs->ws_closed = 1;
                        free(frame.payload);
                        fprintf(stderr, "wsReceive: protocol error: invalid close code %u\n", (unsigned)code);
                        exit(1);
                    }
                    // Validate reason UTF-8 if present.
                    // Strict UTF-8 validation: reject overlong sequences, surrogate
                    // halves (U+D800..U+DFFF), and code points > U+10FFFF to match
                    // Interpreter (std::str::from_utf8) and JS (decode+re-encode).
                    if (frame.payload_len > 2) {
                        size_t rlen = frame.payload_len - 2;
                        unsigned char *reason = frame.payload + 2;
                        size_t i = 0;
                        int utf8_ok = 1;
                        while (i < rlen && utf8_ok) {
                            unsigned char c = reason[i];
                            if (c < 0x80) {
                                i++;
                            } else if ((c & 0xE0) == 0xC0) {
                                // 2-byte: must have 1 continuation, code point >= 0x80
                                if (i + 1 >= rlen || (reason[i+1] & 0xC0) != 0x80) { utf8_ok = 0; break; }
                                uint32_t cp = ((uint32_t)(c & 0x1F) << 6) | (uint32_t)(reason[i+1] & 0x3F);
                                if (cp < 0x80) { utf8_ok = 0; break; } // overlong
                                i += 2;
                            } else if ((c & 0xF0) == 0xE0) {
                                // 3-byte: must have 2 continuations, cp >= 0x800, not surrogate
                                if (i + 2 >= rlen || (reason[i+1] & 0xC0) != 0x80 || (reason[i+2] & 0xC0) != 0x80) { utf8_ok = 0; break; }
                                uint32_t cp = ((uint32_t)(c & 0x0F) << 12) | ((uint32_t)(reason[i+1] & 0x3F) << 6) | (uint32_t)(reason[i+2] & 0x3F);
                                if (cp < 0x800) { utf8_ok = 0; break; } // overlong
                                if (cp >= 0xD800 && cp <= 0xDFFF) { utf8_ok = 0; break; } // surrogate
                                i += 3;
                            } else if ((c & 0xF8) == 0xF0) {
                                // 4-byte: must have 3 continuations, cp >= 0x10000, cp <= 0x10FFFF
                                if (i + 3 >= rlen || (reason[i+1] & 0xC0) != 0x80 || (reason[i+2] & 0xC0) != 0x80 || (reason[i+3] & 0xC0) != 0x80) { utf8_ok = 0; break; }
                                uint32_t cp = ((uint32_t)(c & 0x07) << 18) | ((uint32_t)(reason[i+1] & 0x3F) << 12) | ((uint32_t)(reason[i+2] & 0x3F) << 6) | (uint32_t)(reason[i+3] & 0x3F);
                                if (cp < 0x10000) { utf8_ok = 0; break; } // overlong
                                if (cp > 0x10FFFF) { utf8_ok = 0; break; } // out of range
                                i += 4;
                            } else {
                                utf8_ok = 0; break; // invalid lead byte
                            }
                        }
                        if (!utf8_ok) {
                            unsigned char close_1002[2] = { 0x03, 0xEA };
                            taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, close_1002, 2);
                            if (bs) bs->ws_closed = 1;
                            free(frame.payload);
                            fprintf(stderr, "wsReceive: protocol error: invalid UTF-8 in close reason\n");
                            exit(1);
                        }
                    }
                    // Valid close: echo the code in the reply.
                    unsigned char reply[2] = { (unsigned char)(code >> 8), (unsigned char)(code & 0xFF) };
                    if (bs && !bs->ws_closed) {
                        taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, reply, 2);
                    }
                    if (bs) {
                        bs->ws_closed = 1;
                        bs->ws_close_code = (int64_t)code;
                    }
                    free(frame.payload);
                    return taida_net4_make_lax_ws_frame_empty();
                }
            }

            case WS_FRAME_ERROR:
            default: {
                if (frame.payload) free(frame.payload);
                // Send close frame with protocol error (1002).
                unsigned char close_payload[2] = { 0x03, 0xEA }; // 1002
                taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, close_payload, 2);
                if (bs) bs->ws_closed = 1;
                fprintf(stderr, "wsReceive: protocol error\n");
                exit(1);
            }
        }
    }
}

// ── wsClose(ws, code) → Unit (NET4-4d, v5 revision) ────────────────
// v5: wsClose(ws) or wsClose(ws, code) → Unit.
// 2nd arg (code): 0 = default 1000 (Normal Closure), otherwise explicit close code.
// Valid codes: 1000-4999 excluding reserved 1004, 1005, 1006, 1015.
taida_val taida_net_ws_close(taida_val ws, taida_val code_val) {
    Net3WriterState *w = tl_net3_writer;
    if (!w) {
        fprintf(stderr, "wsClose: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }

    if (!taida_net4_validate_ws_token(ws)) {
        fprintf(stderr, "wsClose: first argument must be the WebSocket connection from wsUpgrade\n");
        exit(1);
    }

    if (w->state != NET3_STATE_WEBSOCKET) {
        fprintf(stderr, "wsClose: not in WebSocket state. Call wsUpgrade first.\n");
        exit(1);
    }

    Net4BodyState *bs = tl_net4_body;

    // Idempotent: no-op if already closed.
    if (bs && bs->ws_closed) {
        return 0; // Unit
    }

    // v5: Determine close code from 2nd argument.
    // code_val is a raw Int (lowering passes 0 for default, or the literal value).
    int64_t close_code_i64 = (int64_t)code_val;

    uint16_t close_code;
    if (close_code_i64 == 0) {
        close_code = 1000; // default: Normal Closure
    } else {
        // Validate close code range.
        if (close_code_i64 < 1000 || close_code_i64 > 4999) {
            fprintf(stderr, "wsClose: close code must be 1000-4999, got %lld\n", (long long)close_code_i64);
            exit(1);
        }
        // Reserved codes that must not be sent.
        if (close_code_i64 == 1004 || close_code_i64 == 1005 || close_code_i64 == 1006 || close_code_i64 == 1015) {
            fprintf(stderr, "wsClose: close code %lld is reserved and cannot be sent\n", (long long)close_code_i64);
            exit(1);
        }
        close_code = (uint16_t)close_code_i64;
    }

    int fd = tl_net3_client_fd;

    // Send close frame with the specified close code.
    unsigned char close_payload[2] = { (unsigned char)(close_code >> 8), (unsigned char)(close_code & 0xFF) };
    taida_net4_write_ws_frame(fd, WS_OPCODE_CLOSE, close_payload, 2);

    if (bs) bs->ws_closed = 1;

    return 0; // Unit
}

// v5: wsCloseCode(ws) → Int (NET5-0d)
// Returns the close code received from the peer's close frame.
// 0 = no close frame received yet, 1005 = no status code, 1000-4999 = peer code.
taida_val taida_net_ws_close_code(taida_val ws) {
    Net3WriterState *w = tl_net3_writer;
    if (!w) {
        fprintf(stderr, "wsCloseCode: can only be called inside a 2-argument httpServe handler\n");
        exit(1);
    }

    if (!taida_net4_validate_ws_token(ws)) {
        fprintf(stderr, "wsCloseCode: first argument must be the WebSocket connection from wsUpgrade\n");
        exit(1);
    }

    if (w->state != NET3_STATE_WEBSOCKET) {
        fprintf(stderr, "wsCloseCode: not in WebSocket state. Call wsUpgrade first.\n");
        exit(1);
    }

    Net4BodyState *bs = tl_net4_body;
    int64_t code = (bs) ? bs->ws_close_code : 0;
    return (taida_val)code;
}

// Validate that the writer argument is a genuine BuchiPack token with
// __writer_id === "__v3_streaming_writer" (parity with Interpreter/JS).
static void taida_net3_validate_writer(taida_val writer, const char *api_name) {
    if (!taida_is_buchi_pack(writer)) {
        fprintf(stderr, "%s: first argument must be the writer provided by httpServe\n", api_name);
        exit(1);
    }
    taida_val id_val = taida_pack_get(writer, taida_str_hash((taida_val)"__writer_id"));
    if (id_val == 0) {
        fprintf(stderr, "%s: first argument must be the writer provided by httpServe\n", api_name);
        exit(1);
    }
    const char *id_str = (const char*)id_val;
    size_t id_len = 0;
    if (!taida_read_cstr_len_safe(id_str, 64, &id_len) ||
        id_len != 21 || memcmp(id_str, "__v3_streaming_writer", 21) != 0) {
        fprintf(stderr, "%s: first argument must be the writer provided by httpServe\n", api_name);
        exit(1);
    }
}

// Create a writer BuchiPack token for 2-arg handler.
// Contains __writer_id sentinel field (parity with Interpreter/JS).
static taida_val taida_net3_create_writer_token(void) {
    taida_val pack = taida_pack_new(1);
    taida_pack_set_hash(pack, 0, taida_str_hash((taida_val)"__writer_id"));
    taida_pack_set(pack, 0, (taida_val)"__v3_streaming_writer");
    taida_pack_set_tag(pack, 0, TAIDA_TAG_STR);
    return pack;
}

// ── NET2-5c: Thread pool structures ─────────────────────────────
// Shared state for the thread pool: a mutex-protected queue of client fds.
// Each worker thread pulls a client fd, processes the keep-alive loop, then
// returns to wait for the next fd.

typedef struct {
    int client_fd;
    struct sockaddr_in peer_addr;
} NetClientSlot;

typedef struct {
    // Shared mutable state (protected by mutex)
    pthread_mutex_t mutex;
    pthread_cond_t  cond_available;  // signal workers: new fd or shutdown
    pthread_cond_t  cond_done;       // signal main: a worker finished

    // Queue of pending client fds
    NetClientSlot *queue;
    int queue_cap;
    int queue_head;
    int queue_tail;
    int queue_count;

    // Global request counter (atomic via mutex)
    int64_t request_count;
    int64_t max_requests;

    // Active connection count (for maxConnections enforcement)
    int active_connections;

    // Shutdown flag
    int shutdown;

    // Handler and timeout
    taida_val handler;
    int64_t timeout_ms;

    // NET3-5a: handler arity (1 = one-shot, 2 = streaming, -1 = unknown/runtime detect)
    int handler_arity;

    // NET5-4a: TLS context (NULL = plaintext, non-NULL = TLS).
    OSSL_SSL_CTX *ssl_ctx;
} NetThreadPool;

static void net_pool_init(NetThreadPool *pool, int queue_cap, taida_val handler, int64_t max_requests, int64_t timeout_ms, int handler_arity) {
    pthread_mutex_init(&pool->mutex, NULL);
    pthread_cond_init(&pool->cond_available, NULL);
    pthread_cond_init(&pool->cond_done, NULL);
    pool->queue_cap = queue_cap;
    pool->queue = (NetClientSlot*)TAIDA_MALLOC(sizeof(NetClientSlot) * (size_t)queue_cap, "net_pool_queue");
    pool->queue_head = 0;
    pool->queue_tail = 0;
    pool->queue_count = 0;
    pool->request_count = 0;
    pool->max_requests = max_requests;
    pool->active_connections = 0;
    pool->shutdown = 0;
    pool->handler = handler;
    pool->timeout_ms = timeout_ms;
    pool->handler_arity = handler_arity;
    pool->ssl_ctx = NULL; // NET5-4a: set by httpServe if TLS configured
}

static void net_pool_destroy(NetThreadPool *pool) {
    pthread_mutex_destroy(&pool->mutex);
    pthread_cond_destroy(&pool->cond_available);
    pthread_cond_destroy(&pool->cond_done);
    free(pool->queue);
}

// Enqueue a client fd. Returns 0 on success, -1 if queue full.
static int net_pool_enqueue(NetThreadPool *pool, int fd, struct sockaddr_in addr) {
    if (pool->queue_count >= pool->queue_cap) return -1;
    pool->queue[pool->queue_tail].client_fd = fd;
    pool->queue[pool->queue_tail].peer_addr = addr;
    pool->queue_tail = (pool->queue_tail + 1) % pool->queue_cap;
    pool->queue_count++;
    return 0;
}

// Dequeue a client fd. Returns 0 on success, -1 if empty.
static int net_pool_dequeue(NetThreadPool *pool, NetClientSlot *out) {
    if (pool->queue_count <= 0) return -1;
    *out = pool->queue[pool->queue_head];
    pool->queue_head = (pool->queue_head + 1) % pool->queue_cap;
    pool->queue_count--;
    return 0;
}

// Check if the global request limit has been reached (call under mutex).
static int net_pool_requests_exhausted(NetThreadPool *pool) {
    return (pool->max_requests > 0 && pool->request_count >= pool->max_requests);
}

// ── NET2-5a/5b/5c: Worker thread — keep-alive loop per connection ──
static void *net_worker_thread(void *arg) {
    NetThreadPool *pool = (NetThreadPool*)arg;

    for (;;) {
        NetClientSlot slot;

        // Wait for a client fd or shutdown
        pthread_mutex_lock(&pool->mutex);
        while (pool->queue_count == 0 && !pool->shutdown) {
            pthread_cond_wait(&pool->cond_available, &pool->mutex);
        }
        if (pool->shutdown && pool->queue_count == 0) {
            pthread_mutex_unlock(&pool->mutex);
            break;
        }
        net_pool_dequeue(pool, &slot);
        pool->active_connections++;
        pthread_mutex_unlock(&pool->mutex);

        int client_fd = slot.client_fd;
        struct sockaddr_in peer_addr = slot.peer_addr;

        char host_buf[INET_ADDRSTRLEN] = {0};
        const char *peer_host = inet_ntop(AF_INET, &peer_addr.sin_addr, host_buf, sizeof(host_buf));
        if (!peer_host) peer_host = "";
        int peer_port = (int)ntohs(peer_addr.sin_port);

        // Set read timeout on client socket
        int64_t effective_timeout = (pool->timeout_ms > 0) ? pool->timeout_ms : 5000;
        {
            struct timeval tv;
            tv.tv_sec = (long)(effective_timeout / 1000);
            tv.tv_usec = (long)((effective_timeout % 1000) * 1000);
            setsockopt(client_fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
            // Also set write timeout for TLS handshake and writes.
            setsockopt(client_fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
        }

        // NET5-4a: TLS handshake if pool has SSL_CTX.
        OSSL_SSL *conn_ssl = NULL;
        if (pool->ssl_ctx) {
            conn_ssl = taida_tls_handshake(pool->ssl_ctx, client_fd);
            if (!conn_ssl) {
                // NET5-0c: handshake failure = close connection, don't call handler.
                close(client_fd);
                pthread_mutex_lock(&pool->mutex);
                pool->active_connections--;
                pthread_cond_signal(&pool->cond_done);
                pthread_mutex_unlock(&pool->mutex);
                continue;
            }
        }
        tl_ssl = conn_ssl;

        // Per-connection scratch buffer (allocated once, reused via advance)
        #define NET_MAX_REQUEST_BUF 1048576
        size_t buf_cap = 8192;
        unsigned char *buf = (unsigned char*)TAIDA_MALLOC(buf_cap, "net_worker_buf");
        size_t total_read = 0;

        // ── Keep-alive loop ──
        for (;;) {

            // Phase 1: Read until HTTP head is complete
            // NB2-19: Parse once, reuse result for keepAlive + request pack building.
            // NB2-9: Properly release parse_result / parse_bytes to prevent memory leak.
            int head_complete = 0;
            size_t head_consumed = 0;
            int64_t content_length = 0;
            int is_chunked = 0;
            int head_malformed = 0;
            taida_val parse_result = 0;  // retained across head+body for single-parse reuse
            taida_val parse_inner = 0;   // inner pack from parse_result

            while (total_read < NET_MAX_REQUEST_BUF) {
                // Try to parse what we have so far
                if (total_read > 3) {
                    int found_end = 0;
                    for (size_t i = 0; i + 3 < total_read; i++) {
                        if (buf[i] == '\r' && buf[i+1] == '\n' && buf[i+2] == '\r' && buf[i+3] == '\n') {
                            found_end = 1;
                            head_consumed = i + 4;
                            break;
                        }
                    }
                    if (found_end) {
                        head_complete = 1;
                        taida_val parse_bytes = taida_bytes_from_raw(buf, (taida_val)total_read);
                        parse_result = taida_net_http_parse_request_head(parse_bytes);
                        taida_val throw_val = taida_pack_get(parse_result, taida_str_hash((taida_val)"throw"));
                        if (throw_val != 0) {
                            head_malformed = 1;
                            taida_release(parse_bytes);
                            taida_release(parse_result);
                            parse_result = 0;
                            break;
                        }
                        parse_inner = taida_pack_get(parse_result, taida_str_hash((taida_val)"__value"));
                        if (parse_inner != 0 && taida_is_buchi_pack(parse_inner)) {
                            content_length = taida_pack_get(parse_inner, taida_str_hash((taida_val)"contentLength"));
                            head_consumed = (size_t)taida_pack_get(parse_inner, taida_str_hash((taida_val)"consumed"));
                            taida_val chunked_val = taida_pack_get(parse_inner, taida_str_hash((taida_val)"chunked"));
                            is_chunked = (chunked_val != 0) ? 1 : 0;
                        }
                        taida_release(parse_bytes);
                        break;
                    }
                }

                // Read more data
                if (total_read >= buf_cap) {
                    size_t new_cap = buf_cap * 2;
                    if (new_cap > NET_MAX_REQUEST_BUF) new_cap = NET_MAX_REQUEST_BUF;
                    TAIDA_REALLOC(buf, new_cap, "net_worker_head");
                    buf_cap = new_cap;
                }
                ssize_t n = taida_tls_recv(client_fd, buf + total_read, buf_cap - total_read);
                if (n <= 0) {
                    // EOF or timeout — partial head gets 400 (parity with interpreter)
                    if (total_read > 0) {
                        const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        taida_net_send_all(client_fd, bad, strlen(bad));
                        pthread_mutex_lock(&pool->mutex);
                        if (!net_pool_requests_exhausted(pool)) {
                            pool->request_count++;
                        }
                        pthread_mutex_unlock(&pool->mutex);
                    }
                    goto conn_done;
                }
                total_read += (size_t)n;
            }

            if (!head_complete || head_malformed) {
                const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                taida_net_send_all(client_fd, bad, strlen(bad));
                pthread_mutex_lock(&pool->mutex);
                if (!net_pool_requests_exhausted(pool)) {
                    pool->request_count++;
                }
                pthread_mutex_unlock(&pool->mutex);
                break; // close connection
            }

            // Head is complete — this counts as a real request.
            pthread_mutex_lock(&pool->mutex);
            if (net_pool_requests_exhausted(pool)) {
                pthread_mutex_unlock(&pool->mutex);
                const char *unavail = "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                taida_net_send_all(client_fd, unavail, strlen(unavail));
                if (parse_result) taida_release(parse_result);
                goto conn_done;
            }
            pool->request_count++;
            pthread_mutex_unlock(&pool->mutex);

            // NET4: Detect handler arity before body reading.
            // 2-arg handler = body-deferred (v4), 1-arg = eager body read (v2).
            int keep_alive = 1;
            size_t wire_consumed = head_consumed; // default for 2-arg deferred
            int skip_buffer_advance = 0; // NB5-24: set by 2-arg path to skip shared advance

            if (pool->handler_arity >= 2) {
                // ── v4 2-arg handler path: body-deferred ──
                // Do NOT eagerly read body. raw = head only.

                // Determine keep-alive from head.
                taida_val http_minor = 1;
                taida_val parsed_headers = 0;
                if (parse_inner != 0 && taida_is_buchi_pack(parse_inner)) {
                    taida_val ver = taida_pack_get(parse_inner, taida_str_hash((taida_val)"version"));
                    if (ver != 0 && taida_is_buchi_pack(ver)) {
                        http_minor = taida_pack_get(ver, taida_str_hash((taida_val)"minor"));
                    }
                    parsed_headers = taida_pack_get(parse_inner, taida_str_hash((taida_val)"headers"));
                }
                keep_alive = taida_net_determine_keep_alive(buf, head_consumed, parsed_headers, http_minor);

                // Capture leftover body bytes already in buf (beyond head).
                size_t leftover_len = (total_read > head_consumed) ? (total_read - head_consumed) : 0;
                unsigned char *leftover = NULL;
                if (leftover_len > 0) {
                    leftover = (unsigned char*)TAIDA_MALLOC(leftover_len, "net_v4_leftover");
                    memcpy(leftover, buf + head_consumed, leftover_len);
                }

                // Create body streaming state.
                Net4BodyState body_state;
                memset(&body_state, 0, sizeof(body_state));
                body_state.is_chunked = is_chunked;
                body_state.content_length = content_length;
                body_state.bytes_consumed = 0;
                body_state.fully_read = (!is_chunked && content_length == 0) ? 1 : 0;
                body_state.any_read_started = 0;
                body_state.leftover = leftover;
                body_state.leftover_len = leftover_len;
                body_state.leftover_pos = 0;
                body_state.chunked_state = NET4_CHUNKED_WAIT_SIZE;
                body_state.chunked_remaining = 0;
                body_state.request_token = taida_net4_alloc_token();
                body_state.ws_closed = 0;
                body_state.ws_token = 0;
                body_state.ws_close_code = 0; // v5: no close frame received yet

                // Build request pack (head only, body = empty span).
                taida_val raw_bytes = taida_bytes_from_raw(buf, (taida_val)head_consumed);
                // 15 fields: raw, method, path, query, version, headers, body, bodyOffset,
                //            contentLength, remoteHost, remotePort, keepAlive, chunked,
                //            __body_stream, __body_token
                taida_val request = taida_pack_new(15);
                taida_pack_set_hash(request, 0, taida_str_hash((taida_val)"raw"));
                taida_pack_set(request, 0, raw_bytes);
                taida_pack_set_tag(request, 0, TAIDA_TAG_PACK);

                if (parse_inner != 0 && taida_is_buchi_pack(parse_inner)) {
                    taida_val method_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"method"));
                    taida_pack_set_hash(request, 1, taida_str_hash((taida_val)"method"));
                    taida_pack_set(request, 1, method_v);
                    taida_pack_set_tag(request, 1, TAIDA_TAG_PACK);
                    if (method_v > 4096) taida_retain(method_v);

                    taida_val path_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"path"));
                    taida_pack_set_hash(request, 2, taida_str_hash((taida_val)"path"));
                    taida_pack_set(request, 2, path_v);
                    taida_pack_set_tag(request, 2, TAIDA_TAG_PACK);
                    if (path_v > 4096) taida_retain(path_v);

                    taida_val query_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"query"));
                    taida_pack_set_hash(request, 3, taida_str_hash((taida_val)"query"));
                    taida_pack_set(request, 3, query_v);
                    taida_pack_set_tag(request, 3, TAIDA_TAG_PACK);
                    if (query_v > 4096) taida_retain(query_v);

                    taida_val version_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"version"));
                    taida_pack_set_hash(request, 4, taida_str_hash((taida_val)"version"));
                    taida_pack_set(request, 4, version_v);
                    taida_pack_set_tag(request, 4, TAIDA_TAG_PACK);
                    if (version_v > 4096) taida_retain(version_v);

                    taida_val headers_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"headers"));
                    taida_pack_set_hash(request, 5, taida_str_hash((taida_val)"headers"));
                    taida_pack_set(request, 5, headers_v);
                    taida_pack_set_tag(request, 5, TAIDA_TAG_LIST);
                    if (headers_v > 4096) taida_retain(headers_v);
                } else {
                    taida_pack_set_hash(request, 1, taida_str_hash((taida_val)"method"));
                    taida_pack_set(request, 1, taida_net_make_span(0, 0));
                    taida_pack_set_tag(request, 1, TAIDA_TAG_PACK);
                    taida_pack_set_hash(request, 2, taida_str_hash((taida_val)"path"));
                    taida_pack_set(request, 2, taida_net_make_span(0, 0));
                    taida_pack_set_tag(request, 2, TAIDA_TAG_PACK);
                    taida_pack_set_hash(request, 3, taida_str_hash((taida_val)"query"));
                    taida_pack_set(request, 3, taida_net_make_span(0, 0));
                    taida_pack_set_tag(request, 3, TAIDA_TAG_PACK);
                    taida_val ver = taida_pack_new(2);
                    taida_pack_set_hash(ver, 0, taida_str_hash((taida_val)"major"));
                    taida_pack_set(ver, 0, 1);
                    taida_pack_set_hash(ver, 1, taida_str_hash((taida_val)"minor"));
                    taida_pack_set(ver, 1, 1);
                    taida_pack_set_hash(request, 4, taida_str_hash((taida_val)"version"));
                    taida_pack_set(request, 4, ver);
                    taida_pack_set_tag(request, 4, TAIDA_TAG_PACK);
                    taida_pack_set_hash(request, 5, taida_str_hash((taida_val)"headers"));
                    taida_pack_set(request, 5, taida_list_new());
                    taida_pack_set_tag(request, 5, TAIDA_TAG_LIST);
                }
                // v4: body span is empty (body not yet read).
                taida_pack_set_hash(request, 6, taida_str_hash((taida_val)"body"));
                taida_pack_set(request, 6, taida_net_make_span(0, 0));
                taida_pack_set_tag(request, 6, TAIDA_TAG_PACK);
                taida_pack_set_hash(request, 7, taida_str_hash((taida_val)"bodyOffset"));
                taida_pack_set(request, 7, (taida_val)head_consumed);
                taida_pack_set_hash(request, 8, taida_str_hash((taida_val)"contentLength"));
                taida_pack_set(request, 8, (taida_val)content_length);
                taida_pack_set_hash(request, 9, taida_str_hash((taida_val)"remoteHost"));
                taida_pack_set(request, 9, (taida_val)taida_str_new_copy(peer_host));
                taida_pack_set_tag(request, 9, TAIDA_TAG_STR);
                taida_pack_set_hash(request, 10, taida_str_hash((taida_val)"remotePort"));
                taida_pack_set(request, 10, (taida_val)peer_port);
                taida_pack_set_hash(request, 11, taida_str_hash((taida_val)"keepAlive"));
                taida_pack_set(request, 11, keep_alive ? 1 : 0);
                taida_pack_set_tag(request, 11, TAIDA_TAG_BOOL);
                taida_pack_set_hash(request, 12, taida_str_hash((taida_val)"chunked"));
                taida_pack_set(request, 12, is_chunked ? 1 : 0);
                taida_pack_set_tag(request, 12, TAIDA_TAG_BOOL);
                // v4 sentinel + token.
                taida_pack_set_hash(request, 13, taida_str_hash((taida_val)"__body_stream"));
                taida_pack_set(request, 13, (taida_val)"__v4_body_stream");
                taida_pack_set_tag(request, 13, TAIDA_TAG_STR);
                taida_pack_set_hash(request, 14, taida_str_hash((taida_val)"__body_token"));
                taida_pack_set(request, 14, (taida_val)body_state.request_token);

                if (parse_result) { taida_release(parse_result); parse_result = 0; }

                // Create writer state.
                Net3WriterState writer_state;
                writer_state.state = NET3_STATE_IDLE;
                writer_state.pending_status = 200;
                writer_state.sse_mode = 0;
                writer_state.header_count = 0;

                // Set thread-local context.
                tl_net3_writer = &writer_state;
                tl_net3_client_fd = client_fd;
                tl_net4_body = &body_state;

                taida_val writer_token = taida_net3_create_writer_token();
                taida_val response = taida_invoke_callback2(pool->handler, request, writer_token);

                // Clear thread-local context.
                tl_net3_writer = NULL;
                tl_net3_client_fd = -1;
                tl_net4_body = NULL;

                // ── v4: WebSocket auto-close on handler return ──
                if (writer_state.state == NET3_STATE_WEBSOCKET) {
                    if (!body_state.ws_closed) {
                        unsigned char close_payload[2] = { 0x03, 0xE8 }; // 1000
                        taida_net4_write_ws_frame(client_fd, WS_OPCODE_CLOSE, close_payload, 2);
                    }
                    taida_release(request);
                    taida_release(writer_token);
                    taida_release(response);
                    if (leftover) free(leftover);
                    // WebSocket: never return to keep-alive.
                    // Check request limit.
                    pthread_mutex_lock(&pool->mutex);
                    int limit_hit = net_pool_requests_exhausted(pool);
                    pthread_mutex_unlock(&pool->mutex);
                    total_read = 0;
                    if (limit_hit) {
                        // Signal shutdown.
                        pthread_mutex_lock(&pool->mutex);
                        pool->shutdown = 1;
                        pthread_cond_broadcast(&pool->cond_available);
                        pthread_mutex_unlock(&pool->mutex);
                    }
                    break; // Close connection.
                }

                if (writer_state.state == NET3_STATE_IDLE) {
                    // One-shot fallback.
                    taida_val effective_response = response;
                    int need_default = 1;
                    if (response > 4096 && taida_is_buchi_pack(response)) {
                        taida_val status_val = taida_pack_get(response, taida_str_hash((taida_val)"status"));
                        taida_val body_val = taida_pack_get(response, taida_str_hash((taida_val)"body"));
                        if (status_val != 0 || body_val != 0) need_default = 0;
                    }
                    if (need_default && (response == 0 || !taida_is_buchi_pack(response))) {
                        effective_response = taida_pack_new(3);
                        taida_pack_set_hash(effective_response, 0, taida_str_hash((taida_val)"status"));
                        taida_pack_set(effective_response, 0, 200);
                        taida_pack_set_hash(effective_response, 1, taida_str_hash((taida_val)"headers"));
                        taida_pack_set(effective_response, 1, taida_list_new());
                        taida_pack_set_tag(effective_response, 1, TAIDA_TAG_LIST);
                        taida_pack_set_hash(effective_response, 2, taida_str_hash((taida_val)"body"));
                        taida_pack_set(effective_response, 2, (taida_val)"");
                        taida_pack_set_tag(effective_response, 2, TAIDA_TAG_STR);
                    }
                    // NB6-1: Scatter-gather send — head and body as separate buffers.
                    if (taida_net_send_response_scatter(client_fd, effective_response) != 0) {
                        const char *fallback = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        taida_net_send_all(client_fd, fallback, strlen(fallback));
                    }
                    if (need_default && effective_response != response) taida_release(effective_response);
                } else {
                    // Streaming was started.
                    if (writer_state.state != NET3_STATE_ENDED) {
                        int auto_end_failed = 0;
                        if (writer_state.state == NET3_STATE_HEAD_PREPARED) {
                            if (taida_net3_commit_head(client_fd, &writer_state) != 0) {
                                fprintf(stderr, "httpServe: failed to commit response head during auto-end\n");
                                auto_end_failed = 1;
                            }
                        }
                        if (!auto_end_failed && !taida_net3_is_bodyless_status(writer_state.pending_status)) {
                            taida_net_send_all(client_fd, "0\r\n\r\n", 5);
                        }
                        writer_state.state = NET3_STATE_ENDED;
                        if (auto_end_failed) {
                            // Force connection close
                            keep_alive = 0;
                        }
                    }
                }

                taida_release(request);
                taida_release(writer_token);
                taida_release(response);

                // NET4-1g: If body not fully read, do NOT return to keep-alive.
                int body_done = body_state.fully_read || (!is_chunked && content_length == 0);
                if (!body_done) keep_alive = 0;

                // NB5-24: Recover trailing bytes from body_state leftover.
                // When a pipelined client sends the next request in the same TCP
                // segment as the current body, those bytes end up in leftover beyond
                // the body data. Copy them back into the connection buffer so the
                // keep-alive loop can parse the next request from them.
                size_t trailing_len = 0;
                if (body_state.leftover && body_state.leftover_pos < body_state.leftover_len) {
                    trailing_len = body_state.leftover_len - body_state.leftover_pos;
                }
                if (trailing_len > 0 && keep_alive) {
                    if (trailing_len > buf_cap) {
                        buf_cap = trailing_len > 8192 ? trailing_len : 8192;
                        free(buf);
                        buf = (unsigned char*)TAIDA_MALLOC(buf_cap, "net_worker_buf");
                    }
                    memcpy(buf, body_state.leftover + body_state.leftover_pos, trailing_len);
                    total_read = trailing_len;
                } else {
                    total_read = 0;
                }

                // NB5-24: Skip the shared "Buffer advance" section — the 2-arg path
                // manages its own buffer state (total_read already set correctly).
                skip_buffer_advance = 1;

                if (leftover) free(leftover);
            } else {
                // ── v2/v3 1-arg handler path (unchanged eager body read) ──
                size_t body_start;
                size_t body_len;
                int64_t final_content_length;

                if (is_chunked) {
                    for (;;) {
                        int64_t check = taida_net_chunked_body_complete(buf, total_read, head_consumed);
                        if (check >= 0) break;
                        if (check == -2) {
                            const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            taida_net_send_all(client_fd, bad, strlen(bad));
                            if (parse_result) taida_release(parse_result);
                            goto conn_done;
                        }
                        if (total_read >= NET_MAX_REQUEST_BUF) {
                            const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            taida_net_send_all(client_fd, bad, strlen(bad));
                            if (parse_result) taida_release(parse_result);
                            goto conn_done;
                        }
                        if (total_read >= buf_cap) {
                            size_t new_cap = buf_cap * 2;
                            if (new_cap > NET_MAX_REQUEST_BUF) new_cap = NET_MAX_REQUEST_BUF;
                            TAIDA_REALLOC(buf, new_cap, "net_worker_chunked");
                            buf_cap = new_cap;
                        }
                        ssize_t n = taida_tls_recv(client_fd, buf + total_read, buf_cap - total_read);
                        if (n <= 0) {
                            const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                            taida_net_send_all(client_fd, bad, strlen(bad));
                            if (parse_result) taida_release(parse_result);
                            goto conn_done;
                        }
                        total_read += (size_t)n;
                    }
                    ChunkedCompactResult compact;
                    if (taida_net_chunked_in_place_compact(buf, head_consumed, &compact) < 0) {
                        const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        taida_net_send_all(client_fd, bad, strlen(bad));
                        if (parse_result) taida_release(parse_result);
                        goto conn_done;
                    }
                    wire_consumed = head_consumed + compact.wire_consumed;
                    body_start = head_consumed;
                    body_len = compact.body_len;
                    final_content_length = (int64_t)compact.body_len;
                } else {
                    if (head_consumed + (size_t)content_length > NET_MAX_REQUEST_BUF) {
                        const char *too_large = "HTTP/1.1 413 Content Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        taida_net_send_all(client_fd, too_large, strlen(too_large));
                        if (parse_result) taida_release(parse_result);
                        break;
                    }
                    size_t body_needed = head_consumed + (size_t)content_length;
                    while (total_read < body_needed && total_read < NET_MAX_REQUEST_BUF) {
                        if (total_read >= buf_cap) {
                            size_t new_cap = buf_cap * 2;
                            if (new_cap > NET_MAX_REQUEST_BUF) new_cap = NET_MAX_REQUEST_BUF;
                            TAIDA_REALLOC(buf, new_cap, "net_worker_body");
                            buf_cap = new_cap;
                        }
                        ssize_t n = taida_tls_recv(client_fd, buf + total_read, buf_cap - total_read);
                        if (n <= 0) break;
                        total_read += (size_t)n;
                    }
                    if (content_length > 0 && total_read < body_needed) {
                        const char *bad = "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        taida_net_send_all(client_fd, bad, strlen(bad));
                        if (parse_result) taida_release(parse_result);
                        break;
                    }
                    wire_consumed = body_needed;
                    body_start = head_consumed;
                    body_len = (size_t)content_length;
                    final_content_length = content_length;
                }

                size_t raw_len = is_chunked ? (head_consumed + body_len) : wire_consumed;
                taida_val http_minor = 1;
                taida_val parsed_headers = 0;
                if (parse_inner != 0 && taida_is_buchi_pack(parse_inner)) {
                    taida_val ver = taida_pack_get(parse_inner, taida_str_hash((taida_val)"version"));
                    if (ver != 0 && taida_is_buchi_pack(ver)) {
                        http_minor = taida_pack_get(ver, taida_str_hash((taida_val)"minor"));
                    }
                    parsed_headers = taida_pack_get(parse_inner, taida_str_hash((taida_val)"headers"));
                }
                keep_alive = taida_net_determine_keep_alive(buf, raw_len, parsed_headers, http_minor);

                taida_val raw_bytes = taida_bytes_from_raw(buf, (taida_val)raw_len);
                taida_val request = taida_pack_new(13);
                taida_pack_set_hash(request, 0, taida_str_hash((taida_val)"raw"));
                taida_pack_set(request, 0, raw_bytes);
                taida_pack_set_tag(request, 0, TAIDA_TAG_PACK);

                if (parse_inner != 0 && taida_is_buchi_pack(parse_inner)) {
                    taida_val method_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"method"));
                    taida_pack_set_hash(request, 1, taida_str_hash((taida_val)"method"));
                    taida_pack_set(request, 1, method_v);
                    taida_pack_set_tag(request, 1, TAIDA_TAG_PACK);
                    if (method_v > 4096) taida_retain(method_v);
                    taida_val path_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"path"));
                    taida_pack_set_hash(request, 2, taida_str_hash((taida_val)"path"));
                    taida_pack_set(request, 2, path_v);
                    taida_pack_set_tag(request, 2, TAIDA_TAG_PACK);
                    if (path_v > 4096) taida_retain(path_v);
                    taida_val query_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"query"));
                    taida_pack_set_hash(request, 3, taida_str_hash((taida_val)"query"));
                    taida_pack_set(request, 3, query_v);
                    taida_pack_set_tag(request, 3, TAIDA_TAG_PACK);
                    if (query_v > 4096) taida_retain(query_v);
                    taida_val version_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"version"));
                    taida_pack_set_hash(request, 4, taida_str_hash((taida_val)"version"));
                    taida_pack_set(request, 4, version_v);
                    taida_pack_set_tag(request, 4, TAIDA_TAG_PACK);
                    if (version_v > 4096) taida_retain(version_v);
                    taida_val headers_v = taida_pack_get(parse_inner, taida_str_hash((taida_val)"headers"));
                    taida_pack_set_hash(request, 5, taida_str_hash((taida_val)"headers"));
                    taida_pack_set(request, 5, headers_v);
                    taida_pack_set_tag(request, 5, TAIDA_TAG_LIST);
                    if (headers_v > 4096) taida_retain(headers_v);
                } else {
                    taida_pack_set_hash(request, 1, taida_str_hash((taida_val)"method"));
                    taida_pack_set(request, 1, taida_net_make_span(0, 0));
                    taida_pack_set_tag(request, 1, TAIDA_TAG_PACK);
                    taida_pack_set_hash(request, 2, taida_str_hash((taida_val)"path"));
                    taida_pack_set(request, 2, taida_net_make_span(0, 0));
                    taida_pack_set_tag(request, 2, TAIDA_TAG_PACK);
                    taida_pack_set_hash(request, 3, taida_str_hash((taida_val)"query"));
                    taida_pack_set(request, 3, taida_net_make_span(0, 0));
                    taida_pack_set_tag(request, 3, TAIDA_TAG_PACK);
                    taida_val ver = taida_pack_new(2);
                    taida_pack_set_hash(ver, 0, taida_str_hash((taida_val)"major"));
                    taida_pack_set(ver, 0, 1);
                    taida_pack_set_hash(ver, 1, taida_str_hash((taida_val)"minor"));
                    taida_pack_set(ver, 1, 1);
                    taida_pack_set_hash(request, 4, taida_str_hash((taida_val)"version"));
                    taida_pack_set(request, 4, ver);
                    taida_pack_set_tag(request, 4, TAIDA_TAG_PACK);
                    taida_pack_set_hash(request, 5, taida_str_hash((taida_val)"headers"));
                    taida_pack_set(request, 5, taida_list_new());
                    taida_pack_set_tag(request, 5, TAIDA_TAG_LIST);
                }
                taida_pack_set_hash(request, 6, taida_str_hash((taida_val)"body"));
                taida_pack_set(request, 6, taida_net_make_span((taida_val)body_start, (taida_val)body_len));
                taida_pack_set_tag(request, 6, TAIDA_TAG_PACK);
                taida_pack_set_hash(request, 7, taida_str_hash((taida_val)"bodyOffset"));
                taida_pack_set(request, 7, (taida_val)body_start);
                taida_pack_set_hash(request, 8, taida_str_hash((taida_val)"contentLength"));
                taida_pack_set(request, 8, (taida_val)final_content_length);
                taida_pack_set_hash(request, 9, taida_str_hash((taida_val)"remoteHost"));
                taida_pack_set(request, 9, (taida_val)taida_str_new_copy(peer_host));
                taida_pack_set_tag(request, 9, TAIDA_TAG_STR);
                taida_pack_set_hash(request, 10, taida_str_hash((taida_val)"remotePort"));
                taida_pack_set(request, 10, (taida_val)peer_port);
                taida_pack_set_hash(request, 11, taida_str_hash((taida_val)"keepAlive"));
                taida_pack_set(request, 11, keep_alive ? 1 : 0);
                taida_pack_set_tag(request, 11, TAIDA_TAG_BOOL);
                taida_pack_set_hash(request, 12, taida_str_hash((taida_val)"chunked"));
                taida_pack_set(request, 12, is_chunked ? 1 : 0);
                taida_pack_set_tag(request, 12, TAIDA_TAG_BOOL);

                if (parse_result) { taida_release(parse_result); parse_result = 0; }

                // NB6-1: 1-arg handler — scatter-gather send (head+body separate).
                taida_val response = taida_invoke_callback1(pool->handler, request);
                if (taida_net_send_response_scatter(client_fd, response) != 0) {
                    const char *fallback = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    taida_net_send_all(client_fd, fallback, strlen(fallback));
                }
                taida_release(request);
                taida_release(response);
            }

            // request_count already reserved after head complete — check limit
            pthread_mutex_lock(&pool->mutex);
            int limit_hit = net_pool_requests_exhausted(pool);
            pthread_mutex_unlock(&pool->mutex);

            // Buffer advance: remove consumed bytes, keep any leftover.
            // NB5-24: Skip for 2-arg path — it manages its own buffer state.
            if (!skip_buffer_advance) {
                if (wire_consumed < total_read) {
                    memmove(buf, buf + wire_consumed, total_read - wire_consumed);
                    total_read -= wire_consumed;
                } else {
                    total_read = 0;
                }
            }

            /* D28B-012 (Round 2 wF): per-request arena boundary.
             *
             * Every per-request taida_val (request pack, response pack,
             * span packs, body string, parse_inner subgraph) has been
             * released to refcount 0 above. Their arena-backed slots
             * are logically dead but the bump arena offset has not
             * rewound — that is the 4 GB plateau / 4.7 GB/h drift root
             * cause D28B-012 escalates from C27B-029.
             *
             * `buf` (the per-connection scratch buffer at line 3568) is
             * TAIDA_MALLOC-backed, not arena, so the buffer advance
             * above is unaffected by this reset. The pool->handler
             * closure was created on the main thread (separate __thread
             * arena) and is also unaffected. No live taida_val
             * references this thread's arena across this boundary. */
            taida_arena_request_reset();

            // Close if not keep-alive or limit reached
            if (!keep_alive || limit_hit) break;
        }

    conn_done:
        // NET5-4a: TLS shutdown before closing fd.
        if (conn_ssl) {
            taida_tls_shutdown_free(conn_ssl);
            conn_ssl = NULL;
            tl_ssl = NULL;
        }
        close(client_fd);
        free(buf);
        buf = NULL;
        total_read = 0;
        buf_cap = 8192;

        /* D28B-012 (Round 2 wF): connection-boundary arena reset.
         *
         * Catches the early-exit paths (head_malformed, EOF before
         * head, body parse error, WebSocket close, request limit
         * exhausted on a partial connection) where the per-iteration
         * reset above could not fire. Idempotent if already drained:
         * the freelist drain loops exit immediately on count==0 and
         * the chunk-keep-one-rewind path is a no-op when chunk[0] is
         * already at offset 0. */
        taida_arena_request_reset();

        // Re-allocate buffer for next connection
        // (will be done at top of next keep-alive loop iteration)

        // Decrement active connections and signal main thread
        pthread_mutex_lock(&pool->mutex);
        pool->active_connections--;
        pthread_cond_signal(&pool->cond_done);
        pthread_mutex_unlock(&pool->mutex);

        #undef NET_MAX_REQUEST_BUF
    }

    return NULL;
}

// ── Native HTTP/2 server (NET6-3a: h2 parity with Interpreter) ──────────────
//
// Reference: src/interpreter/net_h2.rs
// Design decisions:
//   - Blocking I/O (single-threaded per-connection, matching the interpreter model)
//   - One connection at a time (accept → serve → next)
//   - Stream multiplexing within a connection (serial handler dispatch)
//   - Connection-local buffers reused across frames
//   - No aggregate frame buffer; head and body are distinct
//   - ALPN "h2" is required (no silent h1 fallback)
//   - h2c (cleartext HTTP/2) is out of scope

// ── H2 constants (mirrors net_h2.rs) ──────────────────────────────────────

#define H2_CONNECTION_PREFACE "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"
#define H2_CONNECTION_PREFACE_LEN 24

#define H2_DEFAULT_INITIAL_WINDOW 65535
#define H2_DEFAULT_MAX_FRAME_SIZE 16384
#define H2_MAX_MAX_FRAME_SIZE     16777215
#define H2_DEFAULT_HEADER_TABLE_SIZE 4096
#define H2_DEFAULT_MAX_CONCURRENT_STREAMS 128
// RFC 9113 Section 6.9.1: flow-control window MUST NOT exceed 2^31-1
#define H2_MAX_FLOW_CONTROL_WINDOW ((int64_t)0x7FFFFFFF)
// Safety limits for HPACK bomb / memory exhaustion protection
#define H2_MAX_CONTINUATION_BUFFER_SIZE (128 * 1024)
#define H2_MAX_DECODED_HEADER_LIST_SIZE (64 * 1024)

// Frame types
#define H2_FRAME_DATA         0x0
#define H2_FRAME_HEADERS      0x1
#define H2_FRAME_PRIORITY     0x2
#define H2_FRAME_RST_STREAM   0x3
#define H2_FRAME_SETTINGS     0x4
#define H2_FRAME_PUSH_PROMISE 0x5
#define H2_FRAME_PING         0x6
#define H2_FRAME_GOAWAY       0x7
#define H2_FRAME_WINDOW_UPDATE 0x8
#define H2_FRAME_CONTINUATION 0x9

// Flags
#define H2_FLAG_END_STREAM  0x1
#define H2_FLAG_ACK         0x1
#define H2_FLAG_END_HEADERS 0x4
#define H2_FLAG_PADDED      0x8
#define H2_FLAG_PRIORITY    0x20

// Error codes
#define H2_ERROR_NO_ERROR          0x0
#define H2_ERROR_PROTOCOL_ERROR    0x1
#define H2_ERROR_INTERNAL_ERROR    0x2
#define H2_ERROR_FLOW_CONTROL_ERROR 0x3
#define H2_ERROR_FRAME_SIZE_ERROR  0x6
#define H2_ERROR_STREAM_CLOSED     0x5
#define H2_ERROR_COMPRESSION_ERROR 0x9

// Settings identifiers
#define H2_SETTINGS_HEADER_TABLE_SIZE      0x1
#define H2_SETTINGS_ENABLE_PUSH            0x2
#define H2_SETTINGS_MAX_CONCURRENT_STREAMS 0x3
#define H2_SETTINGS_INITIAL_WINDOW_SIZE    0x4
#define H2_SETTINGS_MAX_FRAME_SIZE         0x5
#define H2_SETTINGS_MAX_HEADER_LIST_SIZE   0x6

// ── H2 HPACK static table (RFC 7541 Appendix A) ───────────────────────────

typedef struct {
    const char *name;
    const char *value;
} H2HpackStaticEntry;

static const H2HpackStaticEntry H2_STATIC_TABLE[] = {
    { "", "" },                            // 0: unused
    { ":authority", "" },                  // 1
    { ":method", "GET" },                  // 2
    { ":method", "POST" },                 // 3
    { ":path", "/" },                      // 4
    { ":path", "/index.html" },            // 5
    { ":scheme", "http" },                 // 6
    { ":scheme", "https" },                // 7
    { ":status", "200" },                  // 8
    { ":status", "204" },                  // 9
    { ":status", "206" },                  // 10
    { ":status", "304" },                  // 11
    { ":status", "400" },                  // 12
    { ":status", "404" },                  // 13
    { ":status", "500" },                  // 14
    { "accept-charset", "" },              // 15
    { "accept-encoding", "gzip, deflate" },// 16
    { "accept-language", "" },             // 17
    { "accept-ranges", "" },               // 18
    { "accept", "" },                      // 19
    { "access-control-allow-origin", "" }, // 20
    { "age", "" },                         // 21
    { "allow", "" },                       // 22
    { "authorization", "" },               // 23
    { "cache-control", "" },               // 24
    { "content-disposition", "" },         // 25
    { "content-encoding", "" },            // 26
    { "content-language", "" },            // 27
    { "content-length", "" },              // 28
    { "content-location", "" },            // 29
    { "content-range", "" },               // 30
    { "content-type", "" },                // 31
    { "cookie", "" },                      // 32
    { "date", "" },                        // 33
    { "etag", "" },                        // 34
    { "expect", "" },                      // 35
    { "expires", "" },                     // 36
    { "from", "" },                        // 37
    { "host", "" },                        // 38
    { "if-match", "" },                    // 39
    { "if-modified-since", "" },           // 40
    { "if-none-match", "" },               // 41
    { "if-range", "" },                    // 42
    { "if-unmodified-since", "" },         // 43
    { "last-modified", "" },               // 44
    { "link", "" },                        // 45
    { "location", "" },                    // 46
    { "max-forwards", "" },                // 47
    { "proxy-authenticate", "" },          // 48
    { "proxy-authorization", "" },         // 49
    { "range", "" },                       // 50
    { "referer", "" },                     // 51
    { "refresh", "" },                     // 52
    { "retry-after", "" },                 // 53
    { "server", "" },                      // 54
    { "set-cookie", "" },                  // 55
    { "strict-transport-security", "" },   // 56
    { "transfer-encoding", "" },           // 57
    { "user-agent", "" },                  // 58
    { "vary", "" },                        // 59
    { "via", "" },                         // 60
    { "www-authenticate", "" },            // 61
};
#define H2_STATIC_TABLE_LEN (sizeof(H2_STATIC_TABLE) / sizeof(H2_STATIC_TABLE[0]))

// ── H2 HPACK dynamic table ─────────────────────────────────────────────────

typedef struct {
    char *name;
    char *value;
} H2HpackDynEntry;

typedef struct {
    H2HpackDynEntry *entries;  // Ring buffer (newest at index 0 semantics via head/len)
    int cap;                   // Total allocated slots
    int len;                   // Current count
    size_t current_size;       // Current byte size (name + value + 32 each)
    size_t max_size;           // Maximum allowed size
} H2HpackDynTable;

static void h2_dyntable_init(H2HpackDynTable *dt, size_t max_size) {
    dt->entries = NULL;
    dt->cap = 0;
    dt->len = 0;
    dt->current_size = 0;
    dt->max_size = max_size;
}

static void h2_dyntable_free(H2HpackDynTable *dt) {
    for (int i = 0; i < dt->len; i++) {
        free(dt->entries[i].name);
        free(dt->entries[i].value);
    }
    free(dt->entries);
    dt->entries = NULL;
    dt->len = 0;
    dt->cap = 0;
    dt->current_size = 0;
}

static size_t h2_entry_size(const char *name, const char *value) {
    return strlen(name) + strlen(value) + 32;
}

static void h2_dyntable_evict_to_fit(H2HpackDynTable *dt, size_t needed) {
    // NB6-33: Oldest entries are at the front (index 0). Evict from front.
    while (dt->len > 0 && dt->current_size + needed > dt->max_size) {
        dt->current_size -= h2_entry_size(dt->entries[0].name, dt->entries[0].value);
        free(dt->entries[0].name);
        free(dt->entries[0].value);
        // Shift remaining entries left by 1
        dt->len--;
        if (dt->len > 0) {
            memmove(&dt->entries[0], &dt->entries[1], (size_t)dt->len * sizeof(H2HpackDynEntry));
        }
    }
}

static void h2_dyntable_insert(H2HpackDynTable *dt, const char *name, const char *value) {
    size_t sz = h2_entry_size(name, value);
    h2_dyntable_evict_to_fit(dt, sz);
    if (sz > dt->max_size) return; // Entry too large even alone

    // Grow array if needed
    if (dt->len >= dt->cap) {
        int new_cap = dt->cap ? dt->cap * 2 : 8;
        H2HpackDynEntry *new_entries = (H2HpackDynEntry*)realloc(dt->entries,
            (size_t)new_cap * sizeof(H2HpackDynEntry));
        if (!new_entries) return;
        dt->entries = new_entries;
        dt->cap = new_cap;
    }

    // NB6-33: Append at end — O(1) instead of memmove O(n).
    // Newest entries are at the end (index len-1), oldest at front (index 0).
    // NB6-37: Check strdup return values to avoid segfault on OOM.
    char *dup_name = strdup(name);
    char *dup_value = strdup(value);
    if (!dup_name || !dup_value) {
        free(dup_name);
        free(dup_value);
        return;
    }
    dt->entries[dt->len].name = dup_name;
    dt->entries[dt->len].value = dup_value;
    dt->len++;
    dt->current_size += sz;
}

static void h2_dyntable_set_max_size(H2HpackDynTable *dt, size_t new_max) {
    dt->max_size = new_max;
    h2_dyntable_evict_to_fit(dt, 0);
}

// Get entry by 1-based combined index (static + dynamic).
// Returns 0 on success, -1 on out-of-range.
// NB6-33: Dynamic table is stored newest-at-end. HPACK index 0 = newest = entries[len-1].
static int h2_hpack_get_indexed(H2HpackDynTable *dt, size_t index,
                                 const char **name_out, const char **value_out) {
    if (index == 0) return -1;
    if (index < H2_STATIC_TABLE_LEN) {
        *name_out = H2_STATIC_TABLE[index].name;
        *value_out = H2_STATIC_TABLE[index].value;
        return 0;
    }
    size_t dyn_idx = index - H2_STATIC_TABLE_LEN;
    if ((int)dyn_idx >= dt->len) return -1;
    // Map HPACK dynamic index to array position: index 0 = newest = entries[len-1]
    int array_idx = dt->len - 1 - (int)dyn_idx;
    *name_out = dt->entries[array_idx].name;
    *value_out = dt->entries[array_idx].value;
    return 0;
}

static int h2_hpack_get_indexed_name(H2HpackDynTable *dt, size_t index, const char **name_out) {
    const char *v;
    return h2_hpack_get_indexed(dt, index, name_out, &v);
}

// ── H2 HPACK integer coding (RFC 7541 Section 5.1) ────────────────────────

// Decode HPACK integer with prefix_bits prefix.
// Returns bytes consumed, or -1 on error.
static int h2_hpack_decode_int(const unsigned char *data, size_t data_len,
                                uint8_t prefix_bits, size_t *value_out) {
    if (data_len == 0) return -1;
    uint8_t mask = (uint8_t)((1u << prefix_bits) - 1u);
    size_t value = data[0] & mask;
    int pos = 1;
    if (value < (size_t)mask) {
        *value_out = value;
        return pos;
    }
    // Multi-byte
    int shift = 0;
    while (pos < (int)data_len) {
        uint8_t byte = data[pos++];
        value += (size_t)(byte & 0x7F) << shift;
        shift += 7;
        if (!(byte & 0x80)) {
            *value_out = value;
            return pos;
        }
        if (shift > 28) return -1; // overflow guard
    }
    return -1; // truncated
}

// Encode HPACK integer into buf.  Returns bytes written.
static int h2_hpack_encode_int(unsigned char *buf, size_t buf_cap,
                                size_t value, uint8_t prefix_bits, uint8_t prefix_pattern) {
    uint8_t mask = (uint8_t)((1u << prefix_bits) - 1u);
    if (value < (size_t)mask) {
        if (buf_cap < 1) return -1;
        buf[0] = prefix_pattern | (uint8_t)value;
        return 1;
    }
    if (buf_cap < 1) return -1;
    buf[0] = prefix_pattern | mask;
    int pos = 1;
    size_t remaining = value - mask;
    while (remaining >= 128) {
        if (pos >= (int)buf_cap) return -1;
        buf[pos++] = (unsigned char)((remaining & 0x7F) | 0x80);
        remaining >>= 7;
    }
    if (pos >= (int)buf_cap) return -1;
    buf[pos++] = (unsigned char)remaining;
    return pos;
}

// ── H2 HPACK Huffman decode (RFC 7541 Appendix B) ─────────────────────────

// Minimal bit-by-bit Huffman decoder.
// The full table is in net_h2.rs; we duplicate the same data here.
typedef struct { uint8_t sym; uint32_t code; uint8_t bits; } H2HuffEntry;
static const H2HuffEntry H2_HUFFMAN_TABLE[] = {
    { 48, 0x00,  5},{ 49, 0x01,  5},{ 50, 0x02,  5},{ 97, 0x03,  5},
    { 99, 0x04,  5},{101, 0x05,  5},{105, 0x06,  5},{111, 0x07,  5},
    {115, 0x08,  5},{116, 0x09,  5},{ 32, 0x14,  6},{ 37, 0x15,  6},
    { 45, 0x16,  6},{ 46, 0x17,  6},{ 47, 0x18,  6},{ 51, 0x19,  6},
    { 52, 0x1a,  6},{ 53, 0x1b,  6},{ 54, 0x1c,  6},{ 55, 0x1d,  6},
    { 56, 0x1e,  6},{ 57, 0x1f,  6},{ 61, 0x20,  6},{ 65, 0x21,  6},
    { 95, 0x22,  6},{ 98, 0x23,  6},{100, 0x24,  6},{102, 0x25,  6},
    {103, 0x26,  6},{104, 0x27,  6},{108, 0x28,  6},{109, 0x29,  6},
    {110, 0x2a,  6},{112, 0x2b,  6},{114, 0x2c,  6},{117, 0x2d,  6},
    { 58, 0x5c,  7},{ 66, 0x5d,  7},{ 67, 0x5e,  7},{ 68, 0x5f,  7},
    { 69, 0x60,  7},{ 70, 0x61,  7},{ 71, 0x62,  7},{ 72, 0x63,  7},
    { 73, 0x64,  7},{ 74, 0x65,  7},{ 75, 0x66,  7},{ 76, 0x67,  7},
    { 77, 0x68,  7},{ 78, 0x69,  7},{ 79, 0x6a,  7},{ 80, 0x6b,  7},
    { 81, 0x6c,  7},{ 82, 0x6d,  7},{ 83, 0x6e,  7},{ 84, 0x6f,  7},
    { 85, 0x70,  7},{ 86, 0x71,  7},{ 87, 0x72,  7},{ 89, 0x73,  7},
    {106, 0x74,  7},{107, 0x75,  7},{113, 0x76,  7},{118, 0x77,  7},
    {119, 0x78,  7},{120, 0x79,  7},{121, 0x7a,  7},{122, 0x7b,  7},
    { 38, 0xf8,  8},{ 42, 0xf9,  8},{ 44, 0xfa,  8},{ 59, 0xfb,  8},
    { 88, 0xfc,  8},{ 90, 0xfd,  8},{ 33, 0x3f8,10},{ 34, 0x3f9,10},
    { 40, 0x3fa,10},{ 41, 0x3fb,10},{ 63, 0x3fc,10},{ 39, 0x7fa,11},
    { 43, 0x7fb,11},{124, 0x7fc,11},{ 35, 0xffa,12},{ 62, 0xffb,12},
    {  0, 0x1ff8,13},{ 36, 0x1ff9,13},{ 64, 0x1ffa,13},{ 91, 0x1ffb,13},
    { 93, 0x1ffc,13},{126, 0x1ffd,13},{ 94, 0x3ffc,14},{125, 0x3ffd,14},
    { 60, 0x7ffc,15},{ 96, 0x7ffd,15},{123, 0x7ffe,15},{ 92, 0x7fff0,19},
    {195, 0x7fff1,19},{208, 0x7fff2,19},{128, 0xfffe6,20},{130, 0xfffe7,20},
    {131, 0xfffe8,20},{162, 0xfffe9,20},{184, 0xfffea,20},{194, 0xfffeb,20},
    {224, 0xfffec,20},{226, 0xfffed,20},{153, 0x1fffdc,21},{161, 0x1fffdd,21},
    {167, 0x1fffde,21},{172, 0x1fffdf,21},{176, 0x1fffe0,21},{177, 0x1fffe1,21},
    {179, 0x1fffe2,21},{209, 0x1fffe3,21},{216, 0x1fffe4,21},{217, 0x1fffe5,21},
    {227, 0x1fffe6,21},{229, 0x1fffe7,21},{230, 0x1fffe8,21},{129, 0x3fffd2,22},
    {132, 0x3fffd3,22},{133, 0x3fffd4,22},{134, 0x3fffd5,22},{136, 0x3fffd6,22},
    {146, 0x3fffd7,22},{154, 0x3fffd8,22},{156, 0x3fffd9,22},{160, 0x3fffda,22},
    {163, 0x3fffdb,22},{164, 0x3fffdc,22},{169, 0x3fffdd,22},{170, 0x3fffde,22},
    {173, 0x3fffdf,22},{178, 0x3fffe0,22},{181, 0x3fffe1,22},{185, 0x3fffe2,22},
    {186, 0x3fffe3,22},{187, 0x3fffe4,22},{189, 0x3fffe5,22},{190, 0x3fffe6,22},
    {196, 0x3fffe7,22},{198, 0x3fffe8,22},{228, 0x3fffe9,22},{232, 0x3fffea,22},
    {233, 0x3fffeb,22},{  1, 0x7fffd8,23},{135, 0x7fffd9,23},{137, 0x7fffda,23},
    {138, 0x7fffdb,23},{139, 0x7fffdc,23},{140, 0x7fffdd,23},{141, 0x7fffde,23},
    {143, 0x7fffdf,23},{147, 0x7fffe0,23},{149, 0x7fffe1,23},{150, 0x7fffe2,23},
    {151, 0x7fffe3,23},{152, 0x7fffe4,23},{155, 0x7fffe5,23},{157, 0x7fffe6,23},
    {158, 0x7fffe7,23},{165, 0x7fffe8,23},{166, 0x7fffe9,23},{168, 0x7fffea,23},
    {174, 0x7fffeb,23},{175, 0x7fffec,23},{180, 0x7fffed,23},{182, 0x7fffee,23},
    {183, 0x7fffef,23},{188, 0x7ffff0,23},{191, 0x7ffff1,23},{197, 0x7ffff2,23},
    {231, 0x7ffff3,23},{239, 0x7ffff4,23},{  9, 0xffffea,24},{142, 0xffffeb,24},
    {144, 0xffffec,24},{145, 0xffffed,24},{148, 0xffffee,24},{159, 0xffffef,24},
    {171, 0xfffff0,24},{206, 0xfffff1,24},{215, 0xfffff2,24},{225, 0xfffff3,24},
    {236, 0xfffff4,24},{237, 0xfffff5,24},{199, 0x1ffffec,25},{207, 0x1ffffed,25},
    {234, 0x1ffffee,25},{235, 0x1ffffef,25},{192, 0x3ffffdc,26},{193, 0x3ffffdd,26},
    {200, 0x3ffffde,26},{201, 0x3ffffdf,26},{202, 0x3ffffe0,26},{205, 0x3ffffe1,26},
    {210, 0x3ffffe2,26},{213, 0x3ffffe3,26},{218, 0x3ffffe4,26},{219, 0x3ffffe5,26},
    {238, 0x3ffffe6,26},{240, 0x3ffffe7,26},{242, 0x3ffffe8,26},{243, 0x3ffffe9,26},
    {255, 0x3ffffea,26},{203, 0x7ffffd6,27},{204, 0x7ffffd7,27},{211, 0x7ffffd8,27},
    {212, 0x7ffffd9,27},{214, 0x7ffffda,27},{221, 0x7ffffdb,27},{222, 0x7ffffdc,27},
    {223, 0x7ffffdd,27},{241, 0x7ffffde,27},{244, 0x7ffffdf,27},{245, 0x7ffffe0,27},
    {246, 0x7ffffe1,27},{247, 0x7ffffe2,27},{248, 0x7ffffe3,27},{250, 0x7ffffe4,27},
    {251, 0x7ffffe5,27},{252, 0x7ffffe6,27},{253, 0x7ffffe7,27},{254, 0x7ffffe8,27},
    {  2, 0xfffffe2,28},{  3, 0xfffffe3,28},{  4, 0xfffffe4,28},{  5, 0xfffffe5,28},
    {  6, 0xfffffe6,28},{  7, 0xfffffe7,28},{  8, 0xfffffe8,28},{ 11, 0xfffffe9,28},
    { 12, 0xfffffea,28},{ 14, 0xfffffeb,28},{ 15, 0xfffffec,28},{ 16, 0xfffffed,28},
    { 17, 0xfffffee,28},{ 18, 0xfffffef,28},{ 19, 0xffffff0,28},{ 20, 0xffffff1,28},
    { 21, 0xffffff2,28},{ 23, 0xffffff3,28},{ 24, 0xffffff4,28},{ 25, 0xffffff5,28},
    { 26, 0xffffff6,28},{ 27, 0xffffff7,28},{ 28, 0xffffff8,28},{ 29, 0xffffff9,28},
    { 30, 0xffffffa,28},{ 31, 0xffffffb,28},{127, 0xffffffc,28},{220, 0xffffffd,28},
    {249, 0xffffffe,28},{ 10, 0x3ffffffc,30},{ 13, 0x3ffffffd,30},{ 22, 0x3ffffffe,30},
    /* NB7-75: RFC 7541 Section 5.2 — EOS (256) must be in table so decoder can reject it */
    {256, 0x3fffffff,30},
};
#define H2_HUFFMAN_TABLE_LEN (sizeof(H2_HUFFMAN_TABLE)/sizeof(H2_HUFFMAN_TABLE[0]))

// NB6-34: 8-bit prefix lookup table for fast Huffman decode.
// Entries with code length <= 8 are decoded in O(1). Longer codes fall back
// to a reduced linear scan (only entries with bits > 8).
typedef struct {
    uint8_t sym;
    uint8_t bits;  // 0 means no match at this prefix (need longer codes)
} H2HuffLookup;

static H2HuffLookup h2_huff_lut[256];
static int h2_huff_lut_initialized = 0;

// Build the 8-bit lookup table from the Huffman code table.
// Each 8-bit value maps to the symbol decoded by matching the MSBs.
static void h2_huff_build_lut(void) {
    if (h2_huff_lut_initialized) return;
    memset(h2_huff_lut, 0, sizeof(h2_huff_lut));
    for (size_t t = 0; t < H2_HUFFMAN_TABLE_LEN; t++) {
        uint8_t code_len = H2_HUFFMAN_TABLE[t].bits;
        if (code_len == 0 || code_len > 8) continue;
        // Shift code to fill 8-bit prefix, then fill all suffixes
        uint32_t code = H2_HUFFMAN_TABLE[t].code;
        int pad = 8 - code_len;
        uint32_t base = code << pad;
        uint32_t count = (uint32_t)1 << pad;
        for (uint32_t j = 0; j < count; j++) {
            uint32_t idx = base | j;
            if (idx < 256) {
                h2_huff_lut[idx].sym = H2_HUFFMAN_TABLE[t].sym;
                h2_huff_lut[idx].bits = code_len;
            }
        }
    }
    h2_huff_lut_initialized = 1;
}

// Decode a Huffman-encoded byte string into dst.
// Returns decoded byte count, or -1 on error.
static int h2_huffman_decode(const unsigned char *src, size_t src_len,
                              unsigned char *dst, size_t dst_cap) {
    h2_huff_build_lut();
    uint64_t bits = 0;
    uint8_t bits_left = 0;
    int out = 0;

    for (size_t i = 0; i < src_len; i++) {
        bits = (bits << 8) | src[i];
        bits_left += 8;

        while (bits_left >= 5) {
            // Fast path: try 8-bit LUT.
            // When bits_left >= 8, extract the top 8 bits directly.
            // When 5 <= bits_left < 8, left-shift to form an 8-bit prefix
            // and check that the matched code fits within bits_left.
            {
                uint8_t prefix;
                if (bits_left >= 8) {
                    prefix = (uint8_t)(bits >> (bits_left - 8));
                } else {
                    prefix = (uint8_t)(bits << (8 - bits_left));
                }
                H2HuffLookup *entry = &h2_huff_lut[prefix];
                if (entry->bits > 0 && entry->bits <= bits_left) {
                    /* NB7-75: RFC 7541 Section 5.2 — EOS symbol (256) forbidden */
                    if (entry->sym == 256) return -1;
                    if (out >= (int)dst_cap) return -1;
                    dst[out++] = entry->sym;
                    bits_left -= entry->bits;
                    bits &= bits_left ? (((uint64_t)1 << bits_left) - 1) : 0;
                    continue;
                }
            }
            // Slow path: linear scan for codes > 8 bits
            int found = 0;
            for (size_t t = 0; t < H2_HUFFMAN_TABLE_LEN; t++) {
                uint8_t code_len = H2_HUFFMAN_TABLE[t].bits;
                if (code_len <= 8) continue;  // Already handled by LUT
                if (bits_left < code_len) continue;
                uint8_t shift = bits_left - code_len;
                uint32_t candidate = (uint32_t)(bits >> shift);
                if (candidate == H2_HUFFMAN_TABLE[t].code) {
                    /* NB7-75: RFC 7541 Section 5.2 — EOS symbol (256) forbidden */
                    if (H2_HUFFMAN_TABLE[t].sym == 256) return -1;
                    if (out >= (int)dst_cap) return -1;
                    dst[out++] = H2_HUFFMAN_TABLE[t].sym;
                    bits_left -= code_len;
                    bits &= ((uint64_t)1 << bits_left) - 1;
                    found = 1;
                    break;
                }
            }
            if (!found) {
                if (bits_left < 30) break;
                return -1; // invalid
            }
        }
    }
    // Check padding: remaining bits must be 0-7 and all 1s.
    if (bits_left > 7) return -1;
    if (bits_left > 0) {
        uint64_t pad_mask = ((uint64_t)1 << bits_left) - 1;
        if ((bits & pad_mask) != pad_mask) return -1;
    }
    return out;
}

// ── H2 HPACK string coding ─────────────────────────────────────────────────

// Decode an HPACK string (length-prefixed, optionally Huffman).
// Writes null-terminated result into out_buf (up to out_cap-1 bytes).
// Returns total bytes consumed from data, or -1 on error.
static int h2_hpack_decode_string(const unsigned char *data, size_t data_len,
                                   char *out_buf, size_t out_cap) {
    if (data_len == 0) return -1;
    int huffman = (data[0] & 0x80) != 0;
    size_t str_len;
    int consumed = h2_hpack_decode_int(data, data_len, 7, &str_len);
    if (consumed < 0) return -1;
    if ((size_t)consumed + str_len > data_len) return -1;

    const unsigned char *raw = data + consumed;
    if (huffman) {
        int dec_len = h2_huffman_decode(raw, str_len, (unsigned char*)out_buf, out_cap - 1);
        if (dec_len < 0) return -1;
        out_buf[dec_len] = '\0';
    } else {
        if (str_len >= out_cap) return -1;
        memcpy(out_buf, raw, str_len);
        out_buf[str_len] = '\0';
    }
    return consumed + (int)str_len;
}

// Encode a raw (non-Huffman) HPACK string into buf.
// Returns bytes written, or -1 on overflow.
static int h2_hpack_encode_string(unsigned char *buf, size_t buf_cap, const char *s) {
    size_t slen = strlen(s);
    unsigned char int_buf[8];
    int int_sz = h2_hpack_encode_int(int_buf, sizeof(int_buf), slen, 7, 0x00);
    if (int_sz < 0 || (size_t)int_sz + slen > buf_cap) return -1;
    memcpy(buf, int_buf, (size_t)int_sz);
    memcpy(buf + int_sz, s, slen);
    return int_sz + (int)slen;
}

// ── H2 HPACK full header block decode/encode ──────────────────────────────

// NB6-29: Increased from 64 to 128 headers.
// NB6-30: Prevents premature COMPRESSION_ERROR for legitimate many-header requests.
#define H2_MAX_HEADERS 128
// NB6-29: Increased from 4096 to 16384 for value, 256 to 1024 for name.
// Brings Native closer to Interpreter's unlimited dynamic strings while keeping
// bounded memory. Interpreter still enforces MAX_DECODED_HEADER_LIST_SIZE (64KB).
#define H2_HEADER_NAME_SIZE 1024
#define H2_HEADER_BUF_SIZE 16384

typedef struct {
    char name[H2_HEADER_NAME_SIZE];
    char value[H2_HEADER_BUF_SIZE];
} H2Header;

// Decode an HPACK header block.
// Returns number of decoded headers, or -1 on error.
static int h2_hpack_decode_block(const unsigned char *data, size_t data_len,
                                  H2HpackDynTable *dyn,
                                  H2Header *headers, int max_headers) {
    int count = 0;
    size_t pos = 0;

    while (pos < data_len) {
        if (count >= max_headers) return -1;
        uint8_t byte = data[pos];

        if (byte & 0x80) {
            // Indexed header field (Section 6.1)
            size_t index;
            int consumed = h2_hpack_decode_int(data + pos, data_len - pos, 7, &index);
            if (consumed < 0) return -1;
            pos += (size_t)consumed;
            const char *n, *v;
            if (h2_hpack_get_indexed(dyn, index, &n, &v) < 0) return -1;
            snprintf(headers[count].name, sizeof(headers[count].name), "%s", n);
            snprintf(headers[count].value, sizeof(headers[count].value), "%s", v);
            count++;
        } else if (byte & 0x40) {
            // Literal with incremental indexing (Section 6.2.1)
            size_t index;
            int consumed = h2_hpack_decode_int(data + pos, data_len - pos, 6, &index);
            if (consumed < 0) return -1;
            pos += (size_t)consumed;
            char name_buf[H2_HEADER_NAME_SIZE], value_buf[H2_HEADER_BUF_SIZE];
            if (index == 0) {
                int ns = h2_hpack_decode_string(data + pos, data_len - pos, name_buf, sizeof(name_buf));
                if (ns < 0) return -1;
                pos += (size_t)ns;
            } else {
                const char *n;
                if (h2_hpack_get_indexed_name(dyn, index, &n) < 0) return -1;
                snprintf(name_buf, sizeof(name_buf), "%s", n);
            }
            int vs = h2_hpack_decode_string(data + pos, data_len - pos, value_buf, sizeof(value_buf));
            if (vs < 0) return -1;
            pos += (size_t)vs;
            h2_dyntable_insert(dyn, name_buf, value_buf);
            snprintf(headers[count].name, sizeof(headers[count].name), "%s", name_buf);
            snprintf(headers[count].value, sizeof(headers[count].value), "%s", value_buf);
            count++;
        } else if (byte & 0x20) {
            // Dynamic table size update (Section 6.3)
            size_t new_size;
            int consumed = h2_hpack_decode_int(data + pos, data_len - pos, 5, &new_size);
            if (consumed < 0) return -1;
            pos += (size_t)consumed;
            h2_dyntable_set_max_size(dyn, new_size);
        } else {
            // Literal without/never indexing (Sections 6.2.2 / 6.2.3)
            uint8_t prefix = (byte & 0x10) ? 4 : 4;
            size_t index;
            int consumed = h2_hpack_decode_int(data + pos, data_len - pos, prefix, &index);
            if (consumed < 0) return -1;
            pos += (size_t)consumed;
            char name_buf[H2_HEADER_NAME_SIZE], value_buf[H2_HEADER_BUF_SIZE];
            if (index == 0) {
                int ns = h2_hpack_decode_string(data + pos, data_len - pos, name_buf, sizeof(name_buf));
                if (ns < 0) return -1;
                pos += (size_t)ns;
            } else {
                const char *n;
                if (h2_hpack_get_indexed_name(dyn, index, &n) < 0) return -1;
                snprintf(name_buf, sizeof(name_buf), "%s", n);
            }
            int vs = h2_hpack_decode_string(data + pos, data_len - pos, value_buf, sizeof(value_buf));
            if (vs < 0) return -1;
            pos += (size_t)vs;
            snprintf(headers[count].name, sizeof(headers[count].name), "%s", name_buf);
            snprintf(headers[count].value, sizeof(headers[count].value), "%s", value_buf);
            count++;
        }
    }
    return count;
}

// Encode a list of headers into an HPACK block in buf.
// Returns bytes written, or -1 on overflow/error.
static int h2_hpack_encode_block(unsigned char *buf, size_t buf_cap,
                                  H2HpackDynTable *enc_dyn,
                                  const H2Header *headers, int count) {
    int pos = 0;
    for (int i = 0; i < count; i++) {
        const char *name = headers[i].name;
        const char *value = headers[i].value;

        // Try static table exact match
        int exact_idx = -1;
        int name_idx = -1;
        for (int s = 1; s < (int)H2_STATIC_TABLE_LEN; s++) {
            if (strcmp(H2_STATIC_TABLE[s].name, name) == 0) {
                if (name_idx < 0) name_idx = s;
                if (H2_STATIC_TABLE[s].value[0] != '\0' &&
                    strcmp(H2_STATIC_TABLE[s].value, value) == 0) {
                    exact_idx = s;
                    break;
                }
            }
        }

        if (exact_idx > 0) {
            // Indexed header field
            unsigned char tmp[8];
            int n = h2_hpack_encode_int(tmp, sizeof(tmp), (size_t)exact_idx, 7, 0x80);
            if (n < 0 || pos + n > (int)buf_cap) return -1;
            memcpy(buf + pos, tmp, (size_t)n);
            pos += n;
        } else if (name_idx > 0) {
            // Literal with incremental indexing, indexed name
            unsigned char tmp[8];
            int n = h2_hpack_encode_int(tmp, sizeof(tmp), (size_t)name_idx, 6, 0x40);
            if (n < 0 || pos + n > (int)buf_cap) return -1;
            memcpy(buf + pos, tmp, (size_t)n);
            pos += n;
            int vs = h2_hpack_encode_string(buf + pos, buf_cap - (size_t)pos, value);
            if (vs < 0) return -1;
            pos += vs;
            h2_dyntable_insert(enc_dyn, name, value);
        } else {
            // Literal with incremental indexing, new name
            if (pos >= (int)buf_cap) return -1;
            buf[pos++] = 0x40;
            int ns = h2_hpack_encode_string(buf + pos, buf_cap - (size_t)pos, name);
            if (ns < 0) return -1;
            pos += ns;
            int vs = h2_hpack_encode_string(buf + pos, buf_cap - (size_t)pos, value);
            if (vs < 0) return -1;
            pos += vs;
            h2_dyntable_insert(enc_dyn, name, value);
        }
    }
    return pos;
}

// ── H2 stream state ────────────────────────────────────────────────────────

#define H2_STREAM_IDLE              0
#define H2_STREAM_HALF_CLOSED_REMOTE 1
#define H2_STREAM_CLOSED            2

typedef struct {
    uint32_t stream_id;
    int state;
    H2Header *request_headers;
    int request_header_count;
    unsigned char *request_body;
    size_t request_body_len;
    size_t request_body_cap;
    int64_t send_window;
    int64_t recv_window;
} H2Stream;

// Simple stream table (small fixed-size array for the blocking serial model)
#define H2_MAX_STREAMS 256

typedef struct {
    H2Stream streams[H2_MAX_STREAMS];
    int stream_count;
    H2HpackDynTable decoder_dyn;
    H2HpackDynTable encoder_dyn;
    int64_t conn_send_window;
    int64_t conn_recv_window;
    uint32_t peer_max_frame_size;
    uint32_t peer_initial_window_size;
    uint32_t local_max_frame_size;
    uint32_t last_peer_stream_id;
    int goaway_sent;
    // CONTINUATION state
    unsigned char *continuation_buf;
    size_t continuation_len;
    size_t continuation_cap;
    uint32_t continuation_stream_id;
    uint8_t continuation_flags;
} H2Conn;

// NB6-41: Search from end — most recent streams are at higher indices,
// and the hot-path frame loop typically references the latest stream.
static H2Stream *h2_conn_find_stream(H2Conn *conn, uint32_t stream_id) {
    for (int i = conn->stream_count - 1; i >= 0; i--) {
        if (conn->streams[i].stream_id == stream_id) return &conn->streams[i];
    }
    return NULL;
}

static H2Stream *h2_conn_new_stream(H2Conn *conn, uint32_t stream_id) {
    if (conn->stream_count >= H2_MAX_STREAMS) return NULL;
    H2Stream *s = &conn->streams[conn->stream_count++];
    memset(s, 0, sizeof(*s));
    s->stream_id = stream_id;
    s->state = H2_STREAM_IDLE;
    s->request_headers = NULL;
    s->request_header_count = 0;
    s->request_body = NULL;
    s->request_body_len = 0;
    s->request_body_cap = 0;
    s->send_window = (int64_t)conn->peer_initial_window_size;
    s->recv_window = H2_DEFAULT_INITIAL_WINDOW;
    return s;
}

static void h2_stream_free(H2Stream *s) {
    free(s->request_headers);
    s->request_headers = NULL;
    free(s->request_body);
    s->request_body = NULL;
}

static void h2_conn_remove_closed_streams(H2Conn *conn) {
    int new_count = 0;
    for (int i = 0; i < conn->stream_count; i++) {
        if (conn->streams[i].state != H2_STREAM_CLOSED) {
            if (i != new_count) conn->streams[new_count] = conn->streams[i];
            new_count++;
        } else {
            h2_stream_free(&conn->streams[i]);
        }
    }
    conn->stream_count = new_count;
}

static void h2_conn_init(H2Conn *conn) {
    memset(conn, 0, sizeof(*conn));
    h2_dyntable_init(&conn->decoder_dyn, H2_DEFAULT_HEADER_TABLE_SIZE);
    h2_dyntable_init(&conn->encoder_dyn, H2_DEFAULT_HEADER_TABLE_SIZE);
    conn->conn_send_window = H2_DEFAULT_INITIAL_WINDOW;
    conn->conn_recv_window = H2_DEFAULT_INITIAL_WINDOW;
    conn->peer_max_frame_size = H2_DEFAULT_MAX_FRAME_SIZE;
    conn->peer_initial_window_size = H2_DEFAULT_INITIAL_WINDOW;
    conn->local_max_frame_size = H2_DEFAULT_MAX_FRAME_SIZE;
    conn->goaway_sent = 0;
}

static void h2_conn_free(H2Conn *conn) {
    for (int i = 0; i < conn->stream_count; i++) h2_stream_free(&conn->streams[i]);
    conn->stream_count = 0;
    h2_dyntable_free(&conn->decoder_dyn);
    h2_dyntable_free(&conn->encoder_dyn);
    free(conn->continuation_buf);
    conn->continuation_buf = NULL;
    conn->continuation_len = 0;
    conn->continuation_cap = 0;
}

// ── H2 frame I/O helpers ───────────────────────────────────────────────────

// Read exactly n bytes. Returns n on success, 0 on clean EOF, -1 on error.
static int h2_read_exact(int fd, unsigned char *buf, size_t n) {
    size_t pos = 0;
    while (pos < n) {
        ssize_t r = taida_tls_recv(fd, buf + pos, n - pos);
        if (r <= 0) return (r == 0 && pos == 0) ? 0 : -1;
        pos += (size_t)r;
    }
    return (int)n;
}

// Write all bytes. Returns 0 on success, -1 on error.
// taida_tls_send_all returns 0 on success, -1 on error — pass through directly.
static int h2_write_all(int fd, const unsigned char *buf, size_t n) {
    return taida_tls_send_all(fd, buf, n);
}

// Write a single H2 frame (9-byte header + payload).
// frame_type, flags, stream_id, payload/payload_len.
static int h2_write_frame(int fd, uint8_t frame_type, uint8_t flags,
                           uint32_t stream_id, const unsigned char *payload, uint32_t payload_len) {
    unsigned char header[9];
    header[0] = (payload_len >> 16) & 0xFF;
    header[1] = (payload_len >> 8) & 0xFF;
    header[2] = payload_len & 0xFF;
    header[3] = frame_type;
    header[4] = flags;
    header[5] = (stream_id >> 24) & 0x7F;
    header[6] = (stream_id >> 16) & 0xFF;
    header[7] = (stream_id >> 8) & 0xFF;
    header[8] = stream_id & 0xFF;
    if (h2_write_all(fd, header, 9) < 0) return -1;
    if (payload_len > 0 && h2_write_all(fd, payload, (size_t)payload_len) < 0) return -1;
    return 0;
}

// Validate that decoded header list does not exceed safety limit.
// Returns 0 on success, -1 if headers are too large.
// RFC 9113 Section 6.5.2: size = sum of (name_len + value_len + 32) per entry.
static int h2_validate_header_list_size(const H2Header *headers, int count) {
    size_t total = 0;
    for (int i = 0; i < count; i++) {
        total += strlen(headers[i].name) + strlen(headers[i].value) + 32;
        if (total > H2_MAX_DECODED_HEADER_LIST_SIZE) return -1;
    }
    return 0;
}

// Read one frame. Returns 1 on success, 0 on clean close, -1 on error/protocol violation.
// On success, *payload_out is malloc'd (caller must free), *payload_len_out is set.
static int h2_read_frame(int fd, uint32_t max_frame_size,
                          uint8_t *type_out, uint8_t *flags_out, uint32_t *stream_id_out,
                          unsigned char **payload_out, uint32_t *payload_len_out) {
    unsigned char header[9];
    int r = h2_read_exact(fd, header, 9);
    if (r == 0) return 0;
    if (r < 0) return -1;

    uint32_t len = ((uint32_t)header[0] << 16) | ((uint32_t)header[1] << 8) | header[2];
    *type_out = header[3];
    *flags_out = header[4];
    *stream_id_out = (((uint32_t)(header[5] & 0x7F)) << 24) |
                     ((uint32_t)header[6] << 16) |
                     ((uint32_t)header[7] << 8)  |
                      (uint32_t)header[8];
    *payload_len_out = len;

    if (len > max_frame_size) return -2; // FRAME_SIZE_ERROR

    if (len > 0) {
        *payload_out = (unsigned char*)TAIDA_MALLOC((size_t)len, "h2_frame_payload");
        if (!*payload_out) return -1;
        if (h2_read_exact(fd, *payload_out, (size_t)len) != (int)len) {
            free(*payload_out);
            *payload_out = NULL;
            return -1;
        }
    } else {
        *payload_out = NULL;
    }
    return 1;
}

// Send GOAWAY frame (connection-level error/graceful shutdown).
static int h2_send_goaway(int fd, uint32_t last_stream_id,
                           uint32_t error_code, const char *debug_data) {
    size_t debug_len = debug_data ? strlen(debug_data) : 0;
    size_t payload_len = 8 + debug_len;
    unsigned char *payload = (unsigned char*)TAIDA_MALLOC(payload_len, "h2_goaway_payload");
    if (!payload) return -1;
    payload[0] = (last_stream_id >> 24) & 0x7F;
    payload[1] = (last_stream_id >> 16) & 0xFF;
    payload[2] = (last_stream_id >> 8) & 0xFF;
    payload[3] = last_stream_id & 0xFF;
    payload[4] = (error_code >> 24) & 0xFF;
    payload[5] = (error_code >> 16) & 0xFF;
    payload[6] = (error_code >> 8) & 0xFF;
    payload[7] = error_code & 0xFF;
    if (debug_len > 0) memcpy(payload + 8, debug_data, debug_len);
    int rc = h2_write_frame(fd, H2_FRAME_GOAWAY, 0, 0, payload, (uint32_t)payload_len);
    free(payload);
    return rc;
}

// Send RST_STREAM frame.
static int h2_send_rst_stream(int fd, uint32_t stream_id, uint32_t error_code) {
    unsigned char payload[4];
    payload[0] = (error_code >> 24) & 0xFF;
    payload[1] = (error_code >> 16) & 0xFF;
    payload[2] = (error_code >> 8) & 0xFF;
    payload[3] = error_code & 0xFF;
    return h2_write_frame(fd, H2_FRAME_RST_STREAM, 0, stream_id, payload, 4);
}

// Send SETTINGS frame with server defaults.
static int h2_send_server_settings(int fd, uint32_t max_frame_size, uint32_t max_concurrent_streams) {
    unsigned char payload[24]; // 4 settings * 6 bytes each
    int pos = 0;
    // MAX_CONCURRENT_STREAMS
    payload[pos++] = 0x00; payload[pos++] = 0x03;
    payload[pos++] = (max_concurrent_streams >> 24) & 0xFF;
    payload[pos++] = (max_concurrent_streams >> 16) & 0xFF;
    payload[pos++] = (max_concurrent_streams >> 8) & 0xFF;
    payload[pos++] = max_concurrent_streams & 0xFF;
    // INITIAL_WINDOW_SIZE
    payload[pos++] = 0x00; payload[pos++] = 0x04;
    payload[pos++] = 0x00; payload[pos++] = 0x00;
    payload[pos++] = 0xFF; payload[pos++] = 0xFF;
    // MAX_FRAME_SIZE
    payload[pos++] = 0x00; payload[pos++] = 0x05;
    payload[pos++] = (max_frame_size >> 24) & 0xFF;
    payload[pos++] = (max_frame_size >> 16) & 0xFF;
    payload[pos++] = (max_frame_size >> 8) & 0xFF;
    payload[pos++] = max_frame_size & 0xFF;
    // ENABLE_PUSH = 0
    payload[pos++] = 0x00; payload[pos++] = 0x02;
    payload[pos++] = 0x00; payload[pos++] = 0x00; payload[pos++] = 0x00; payload[pos++] = 0x00;
    return h2_write_frame(fd, H2_FRAME_SETTINGS, 0, 0, payload, (uint32_t)pos);
}

// Send SETTINGS ACK.
static int h2_send_settings_ack(int fd) {
    return h2_write_frame(fd, H2_FRAME_SETTINGS, H2_FLAG_ACK, 0, NULL, 0);
}

// Send WINDOW_UPDATE frame.
static int h2_send_window_update(int fd, uint32_t stream_id, uint32_t increment) {
    if (increment == 0 || increment > 0x7FFFFFFF) return -1;
    unsigned char payload[4];
    payload[0] = (increment >> 24) & 0x7F;
    payload[1] = (increment >> 16) & 0xFF;
    payload[2] = (increment >> 8) & 0xFF;
    payload[3] = increment & 0xFF;
    return h2_write_frame(fd, H2_FRAME_WINDOW_UPDATE, 0, stream_id, payload, 4);
}

// Send PING ACK.
static int h2_send_ping_ack(int fd, const unsigned char *opaque, uint32_t opaque_len) {
    return h2_write_frame(fd, H2_FRAME_PING, H2_FLAG_ACK, 0, opaque, opaque_len);
}

// ── H2 response send helpers ──────────────────────────────────────────────

// Send response HEADERS + optional CONTINUATION if the HPACK block is large.
// HPACK encodes ":status" + provided headers into resp_hdr_buf.
// peer_max_frame_size controls frame splitting.
// Returns 0 on success, -1 on error.
static int h2_send_response_headers(int fd, H2HpackDynTable *enc_dyn,
                                     uint32_t stream_id, int status_code,
                                     const H2Header *extra_headers, int extra_count,
                                     int end_stream, uint32_t peer_max_frame_size) {
    // Build header list. Zero-init the full array so cppcheck's
    // legacyUninitvar solver sees every struct member as defined before
    // the snprintf() writes into the first element; in practice the
    // loop below only uses `count` entries, but a cold memset on a
    // stack array this small is below the noise floor.
    H2Header all_headers[H2_MAX_HEADERS];
    memset(all_headers, 0, sizeof(all_headers));
    int count = 0;
    // :status pseudo-header first
    snprintf(all_headers[0].name, sizeof(all_headers[0].name), ":status");
    snprintf(all_headers[0].value, sizeof(all_headers[0].value), "%d", status_code);
    count = 1;
    for (int i = 0; i < extra_count && count < H2_MAX_HEADERS; i++) {
        // Lowercase header names (HTTP/2 requires lowercase)
        size_t nlen = strlen(extra_headers[i].name);
        if (nlen >= sizeof(all_headers[count].name)) nlen = sizeof(all_headers[count].name) - 1;
        for (size_t j = 0; j < nlen; j++) {
            all_headers[count].name[j] = (char)tolower((unsigned char)extra_headers[i].name[j]);
        }
        all_headers[count].name[nlen] = '\0';
        snprintf(all_headers[count].value, sizeof(all_headers[count].value), "%s", extra_headers[i].value);
        count++;
    }

    // NB6-24: Use 8KB stack buffer + heap fallback instead of fixed 64KB malloc.
    // Most response headers are small (< 1KB); 8KB covers typical cases without heap.
    unsigned char hdr_stack[8192];
    size_t hdr_buf_cap = sizeof(hdr_stack);
    unsigned char *hdr_buf = hdr_stack;

    int enc_len = h2_hpack_encode_block(hdr_buf, hdr_buf_cap, enc_dyn,
                                         (const H2Header*)all_headers, count);
    // If stack buffer was too small, retry with heap
    if (enc_len < 0 && hdr_buf == hdr_stack) {
        hdr_buf_cap = 65536;
        hdr_buf = (unsigned char*)TAIDA_MALLOC(hdr_buf_cap, "h2_hdr_block_fallback");
        if (!hdr_buf) return -1;
        enc_len = h2_hpack_encode_block(hdr_buf, hdr_buf_cap, enc_dyn,
                                         (const H2Header*)all_headers, count);
    }
    if (enc_len < 0) { if (hdr_buf != hdr_stack) free(hdr_buf); return -1; }

    uint32_t max_sz = peer_max_frame_size;
    if ((uint32_t)enc_len <= max_sz) {
        // Single HEADERS frame
        uint8_t flags = H2_FLAG_END_HEADERS;
        if (end_stream) flags |= H2_FLAG_END_STREAM;
        int rc = h2_write_frame(fd, H2_FRAME_HEADERS, flags, stream_id, hdr_buf, (uint32_t)enc_len);
        if (hdr_buf != hdr_stack) free(hdr_buf);
        return rc;
    }

    // Split: HEADERS (no END_HEADERS) + CONTINUATION*
    uint8_t flags = 0;
    if (end_stream) flags |= H2_FLAG_END_STREAM;
    if (h2_write_frame(fd, H2_FRAME_HEADERS, flags, stream_id, hdr_buf, max_sz) < 0) {
        if (hdr_buf != hdr_stack) free(hdr_buf); return -1;
    }
    uint32_t offset = max_sz;
    while (offset < (uint32_t)enc_len) {
        uint32_t chunk = (uint32_t)enc_len - offset;
        if (chunk > max_sz) chunk = max_sz;
        int is_last = (offset + chunk >= (uint32_t)enc_len);
        uint8_t cont_flags = is_last ? H2_FLAG_END_HEADERS : 0;
        if (h2_write_frame(fd, H2_FRAME_CONTINUATION, cont_flags, stream_id,
                           hdr_buf + offset, chunk) < 0) {
            if (hdr_buf != hdr_stack) free(hdr_buf); return -1;
        }
        offset += chunk;
    }
    if (hdr_buf != hdr_stack) free(hdr_buf);
    return 0;
}

// Send response DATA frames respecting flow control windows.
// Returns bytes sent, or -1 on error/window exhaustion.
static int64_t h2_send_response_data(int fd, uint32_t stream_id,
                                      const unsigned char *data, size_t data_len,
                                      int end_stream,
                                      uint32_t max_frame_size,
                                      int64_t *conn_send_window,
                                      int64_t *stream_send_window) {
    if (data_len == 0) {
        if (end_stream) h2_write_frame(fd, H2_FRAME_DATA, H2_FLAG_END_STREAM, stream_id, NULL, 0);
        return 0;
    }

    int64_t sent = 0;
    while ((size_t)sent < data_len) {
        size_t remaining = data_len - (size_t)sent;
        size_t frame_limit = (size_t)max_frame_size;
        size_t conn_limit = (*conn_send_window > 0) ? (size_t)*conn_send_window : 0;
        size_t stream_limit = (*stream_send_window > 0) ? (size_t)*stream_send_window : 0;
        size_t chunk = remaining;
        if (chunk > frame_limit) chunk = frame_limit;
        if (chunk > conn_limit) chunk = conn_limit;
        if (chunk > stream_limit) chunk = stream_limit;
        if (chunk == 0) return -1; // window exhausted

        int is_last = ((size_t)sent + chunk >= data_len);
        uint8_t flags = (is_last && end_stream) ? H2_FLAG_END_STREAM : 0;
        if (h2_write_frame(fd, H2_FRAME_DATA, flags, stream_id,
                           data + sent, (uint32_t)chunk) < 0) return -1;
        *conn_send_window -= (int64_t)chunk;
        *stream_send_window -= (int64_t)chunk;
        sent += (int64_t)chunk;
    }
    return sent;
}

// ── H2 frame processing ────────────────────────────────────────────────────

// Process a received SETTINGS frame payload.
static int h2_process_settings(H2Conn *conn, const unsigned char *payload, uint32_t len) {
    if (len % 6 != 0) return -1; // FRAME_SIZE_ERROR
    for (uint32_t i = 0; i + 6 <= len; i += 6) {
        uint16_t id = ((uint16_t)payload[i] << 8) | payload[i+1];
        uint32_t value = ((uint32_t)payload[i+2] << 24) | ((uint32_t)payload[i+3] << 16) |
                         ((uint32_t)payload[i+4] << 8) | payload[i+5];
        switch (id) {
            case H2_SETTINGS_HEADER_TABLE_SIZE:
                h2_dyntable_set_max_size(&conn->encoder_dyn, (size_t)value);
                break;
            case H2_SETTINGS_ENABLE_PUSH:
                if (value > 1) return -1;
                break;
            case H2_SETTINGS_MAX_CONCURRENT_STREAMS:
                // We note it but don't enforce for the blocking serial model
                break;
            case H2_SETTINGS_INITIAL_WINDOW_SIZE:
                if (value > 0x7FFFFFFF) return -1;
                {
                    int64_t delta = (int64_t)value - (int64_t)conn->peer_initial_window_size;
                    conn->peer_initial_window_size = value;
                    for (int s = 0; s < conn->stream_count; s++) {
                        conn->streams[s].send_window += delta;
                    }
                }
                break;
            case H2_SETTINGS_MAX_FRAME_SIZE:
                if (value < H2_DEFAULT_MAX_FRAME_SIZE || value > H2_MAX_MAX_FRAME_SIZE) return -1;
                conn->peer_max_frame_size = value;
                break;
            case H2_SETTINGS_MAX_HEADER_LIST_SIZE:
                break;
            default:
                break; // Unknown settings ignored
        }
    }
    return 0;
}

// ── H2 request extraction from decoded pseudo-headers ─────────────────────

// error_reason values for H2RequestFields (0 = no error)
#define H2_REQ_ERR_NONE            0
#define H2_REQ_ERR_ORDERING        1
#define H2_REQ_ERR_UNKNOWN_PSEUDO  2
#define H2_REQ_ERR_MISSING_PSEUDO  3

typedef struct {
    char method[16];
    char path[2048];
    char authority[256];
    H2Header *regular_headers;
    int regular_count;
    int ok;
    int error_reason;
} H2RequestFields;

// error_reason values for duplicate pseudo-headers
#define H2_REQ_ERR_DUPLICATE_PSEUDO 4
// error_reason values for empty pseudo-header values
#define H2_REQ_ERR_EMPTY_PSEUDO     5
// C27B-026 Step 3 Option B: pseudo-header value exceeds the wire-byte
// upper limit enforced by the parser (mirrors HTTP_WIRE_MAX_* in
// src/interpreter/net_eval/h1.rs and the H1 parser checks above).
// Catches truncation that would otherwise have been silenced by
// snprintf into a fixed-size struct field.
#define H2_REQ_ERR_PSEUDO_TOO_LONG  6

// C27B-026 Step 3 Option B helper: bounded copy via memcpy + pre-
// length check (gcc cannot follow snprintf-with-runtime-check, so
// the -Wformat-truncation warning only stays silent for the memcpy
// form). Mirrors H3_COPY_PSEUDO in net_h3_quic.c. Used inside
// h2_extract_request_fields below.
#define H2_COPY_PSEUDO(dst, dst_size, seen) do { \
    size_t v_len = strlen(headers[i].value); \
    if (v_len >= (dst_size)) { out->error_reason = H2_REQ_ERR_PSEUDO_TOO_LONG; free(regs); return; } \
    memcpy((dst), headers[i].value, v_len); (dst)[v_len] = '\0'; (seen) = 1; \
} while (0)

static void h2_extract_request_fields(const H2Header *headers, int count, H2RequestFields *out) {
    memset(out, 0, sizeof(*out));
    out->regular_headers = NULL;
    out->regular_count = 0;
    out->ok = 0;
    out->error_reason = H2_REQ_ERR_NONE;

    char scheme[16] = "";
    int saw_regular = 0;
    int saw_method = 0, saw_path = 0, saw_authority = 0, saw_scheme = 0;
    H2Header *regs = (H2Header*)TAIDA_MALLOC(sizeof(H2Header) * (size_t)(count + 1), "h2_regular_headers");
    if (!regs) return;
    int reg_count = 0;

    for (int i = 0; i < count; i++) {
        if (headers[i].name[0] == ':') {
            if (saw_regular) {
                out->error_reason = H2_REQ_ERR_ORDERING;
                free(regs);
                return; // ordering violation
            }
            // C27B-026 Step 3 Option B: bounded copy + cap check; see
            // H2_COPY_PSEUDO macro defined above for details.
            // RFC 9113 Section 8.3.1: each pseudo-header MUST NOT appear more than once.
            if (strcmp(headers[i].name, ":method") == 0) {
                if (saw_method) { out->error_reason = H2_REQ_ERR_DUPLICATE_PSEUDO; free(regs); return; }
                H2_COPY_PSEUDO(out->method, sizeof(out->method), saw_method);
            } else if (strcmp(headers[i].name, ":path") == 0) {
                if (saw_path) { out->error_reason = H2_REQ_ERR_DUPLICATE_PSEUDO; free(regs); return; }
                H2_COPY_PSEUDO(out->path, sizeof(out->path), saw_path);
            } else if (strcmp(headers[i].name, ":authority") == 0) {
                if (saw_authority) { out->error_reason = H2_REQ_ERR_DUPLICATE_PSEUDO; free(regs); return; }
                H2_COPY_PSEUDO(out->authority, sizeof(out->authority), saw_authority);
            } else if (strcmp(headers[i].name, ":scheme") == 0) {
                if (saw_scheme) { out->error_reason = H2_REQ_ERR_DUPLICATE_PSEUDO; free(regs); return; }
                H2_COPY_PSEUDO(scheme, sizeof(scheme), saw_scheme);
            } else {
                // Unknown pseudo-header: reject as PROTOCOL_ERROR
                // (matches Interpreter: H2Error::Stream with ERROR_PROTOCOL_ERROR)
                out->error_reason = H2_REQ_ERR_UNKNOWN_PSEUDO;
                free(regs);
                return;
            }
        } else {
            saw_regular = 1;
            if (reg_count < count) {
                regs[reg_count++] = headers[i];
            }
        }
    }

    if (out->method[0] == '\0' || out->path[0] == '\0' || scheme[0] == '\0') {
        out->error_reason = H2_REQ_ERR_MISSING_PSEUDO;
        free(regs);
        return; // missing required pseudo-headers
    }
    out->regular_headers = regs;
    out->regular_count = reg_count;
    out->ok = 1;
}

// ── H2 response extraction from taida_val ─────────────────────────────────
// Mirrors extract_response_fields() in net_eval.rs.

typedef struct {
    int status;
    H2Header *headers;
    int header_count;
    unsigned char *body;
    size_t body_len;
    int ok;
} H2ResponseFields;

static void h2_extract_response_fields(taida_val response, H2ResponseFields *out) {
    memset(out, 0, sizeof(*out));
    out->status = 500;
    out->ok = 0;

    if (!TAIDA_IS_PACK(response)) return;

    // status
    taida_val status_hash = taida_str_hash((taida_val)"status");
    taida_val status_val = taida_pack_get(response, status_hash);
    if (status_val > 0 && status_val < 1000) {
        out->status = (int)status_val;
    } else {
        out->status = 500;
    }

    // headers: @[@(name: Str, value: Str)]
    // C26B-026 fix: `taida_list_get` wraps each entry in a Lax pack
    // (hasValue/__value/__default/__type). The h1 encode path reads raw
    // `hlist[4+i]` to skip the Lax wrapper; mirror that here. Previously we
    // called `taida_list_get(...)` and then `taida_pack_get(entry, "name")`
    // which returned 0 because the Lax pack has no "name" field, causing
    // every custom response header to be silently dropped before HPACK
    // encoding. The response therefore ended up with only `:status` +
    // `content-length`.
    taida_val hdrs_hash = taida_str_hash((taida_val)"headers");
    taida_val hdrs_val = taida_pack_get(response, hdrs_hash);
    int header_cap = H2_MAX_HEADERS;  // parity with h1 serve (64 hdr cap)
    out->headers = (H2Header*)TAIDA_MALLOC(sizeof(H2Header) * (size_t)header_cap, "h2_resp_headers");
    if (!out->headers) return;
    out->header_count = 0;

    if (TAIDA_IS_LIST(hdrs_val)) {
        taida_val *hlist = (taida_val*)hdrs_val;
        int64_t list_len = (int64_t)hlist[2];
        taida_val name_h = taida_str_hash((taida_val)"name");
        taida_val val_h  = taida_str_hash((taida_val)"value");
        for (int64_t j = 0; j < list_len && out->header_count < header_cap; j++) {
            taida_val entry = hlist[4 + j];  // raw inner pack (no Lax wrap)
            if (!TAIDA_IS_PACK(entry)) continue;
            taida_val n = taida_pack_get(entry, name_h);
            taida_val v = taida_pack_get(entry, val_h);
            if (!n || n <= 4096 || !v || v <= 4096) continue;
            snprintf(out->headers[out->header_count].name,
                     sizeof(out->headers[out->header_count].name), "%s", (const char*)n);
            snprintf(out->headers[out->header_count].value,
                     sizeof(out->headers[out->header_count].value), "%s", (const char*)v);
            out->header_count++;
        }
    }

    // body
    taida_val body_hash = taida_str_hash((taida_val)"body");
    taida_val body_val = taida_pack_get(response, body_hash);
    out->body = NULL;
    out->body_len = 0;

    if (body_val && body_val > 4096) {
        // Check if it's Bytes
        taida_val body_tag = taida_pack_get_field_tag(response, body_hash);
        if (body_tag == TAIDA_TAG_UNKNOWN) {
            body_tag = taida_runtime_detect_tag(body_val);
        }
        if (body_tag == TAIDA_TAG_STR) {
            const char *body_str = (const char*)body_val;
            size_t blen = strlen(body_str);
            out->body = (unsigned char*)TAIDA_MALLOC(blen + 1, "h2_resp_body");
            if (out->body) { memcpy(out->body, body_str, blen); out->body_len = blen; }
        } else if (TAIDA_IS_BYTES(body_val)) {
            // Bytes value: header[0]=magic, header[1]=len, then raw bytes inline
            int64_t blen = (int64_t)taida_bytes_len(body_val);
            if (blen > 0) {
                out->body = (unsigned char*)TAIDA_MALLOC((size_t)blen, "h2_resp_body_bytes");
                if (out->body) {
                    // Bytes layout: [magic|refcount, len, b0, b1, ...]
                    taida_val *bdata = (taida_val*)body_val;
                    for (int64_t bi = 0; bi < blen; bi++) {
                        out->body[bi] = (unsigned char)(bdata[2 + bi] & 0xFF);
                    }
                    out->body_len = (size_t)blen;
                }
            }
        }
    }
    out->ok = 1;
}

static void h2_response_fields_free(H2ResponseFields *r) {
    free(r->headers);
    r->headers = NULL;
    free(r->body);
    r->body = NULL;
}

// ── H2 serve one connection ────────────────────────────────────────────────
//
// Processes one HTTP/2 connection: reads frames, dispatches requests,
// sends responses. Returns after the connection closes or max_requests is reached.

typedef struct {
    taida_val handler;
    int handler_arity;
    int64_t *request_count;
    int64_t max_requests;
    char peer_host[64];
    int peer_port;
} H2ServeCtx;

// Call the Taida handler with the request pack and return the response value.
// Uses taida_invoke_callback1 — same calling convention as the h1 1-arg path.
static taida_val h2_dispatch_request(H2ServeCtx *ctx, taida_val request_pack) {
    return taida_invoke_callback1(ctx->handler, request_pack);
}

// Build a taida_val BuchiPack representing the HTTP/2 request.
// This mirrors the Interpreter's request pack in serve_h2().
static taida_val h2_build_request_pack(H2RequestFields *fields,
                                        const unsigned char *body, size_t body_len,
                                        const char *peer_host, int peer_port) {
    // Header list @[@(name: Str, value: Str)]
    taida_val hdr_list = taida_list_new();
    for (int i = 0; i < fields->regular_count; i++) {
        taida_val entry = taida_pack_new(2);
        taida_pack_set_hash(entry, 0, taida_str_hash((taida_val)"name"));
        taida_pack_set(entry, 0, (taida_val)taida_str_new_copy(fields->regular_headers[i].name));
        taida_pack_set_tag(entry, 0, TAIDA_TAG_STR);
        taida_pack_set_hash(entry, 1, taida_str_hash((taida_val)"value"));
        taida_pack_set(entry, 1, (taida_val)taida_str_new_copy(fields->regular_headers[i].value));
        taida_pack_set_tag(entry, 1, TAIDA_TAG_STR);
        hdr_list = taida_list_append(hdr_list, entry);
    }
    // :authority as host header
    if (fields->authority[0] != '\0') {
        taida_val entry = taida_pack_new(2);
        taida_pack_set_hash(entry, 0, taida_str_hash((taida_val)"name"));
        taida_pack_set(entry, 0, (taida_val)taida_str_new_copy("host"));
        taida_pack_set_tag(entry, 0, TAIDA_TAG_STR);
        taida_pack_set_hash(entry, 1, taida_str_hash((taida_val)"value"));
        taida_pack_set(entry, 1, (taida_val)taida_str_new_copy(fields->authority));
        taida_pack_set_tag(entry, 1, TAIDA_TAG_STR);
        hdr_list = taida_list_append(hdr_list, entry);
    }

    // Split path and query
    char path_part[2048], query_part[2048];
    const char *qmark = strchr(fields->path, '?');
    if (qmark) {
        size_t plen = (size_t)(qmark - fields->path);
        if (plen >= sizeof(path_part)) plen = sizeof(path_part) - 1;
        memcpy(path_part, fields->path, plen);
        path_part[plen] = '\0';
        snprintf(query_part, sizeof(query_part), "%s", qmark + 1);
    } else {
        snprintf(path_part, sizeof(path_part), "%s", fields->path);
        query_part[0] = '\0';
    }

    // NB6-26: Build proper Bytes (not List) for raw body — matches Interpreter's Value::Bytes(body)
    taida_val raw_bytes = taida_bytes_from_raw(body, (taida_val)body_len);

    // version pack @(major: 2, minor: 0)
    taida_val version_pack = taida_pack_new(2);
    taida_pack_set_hash(version_pack, 0, taida_str_hash((taida_val)"major"));
    taida_pack_set(version_pack, 0, (taida_val)2);
    taida_pack_set_tag(version_pack, 0, TAIDA_TAG_INT);
    taida_pack_set_hash(version_pack, 1, taida_str_hash((taida_val)"minor"));
    taida_pack_set(version_pack, 1, (taida_val)0);
    taida_pack_set_tag(version_pack, 1, TAIDA_TAG_INT);

    // NB6-28: Request pack: 14 fields (was 13 — missing "chunked")
    // Matches Interpreter's 14-field request pack.
    taida_val req = taida_pack_new(14);
    int f = 0;
    #define SET_FIELD(nm, val, tag) do { \
        taida_pack_set_hash(req, f, taida_str_hash((taida_val)(nm))); \
        taida_pack_set(req, f, (val)); \
        taida_pack_set_tag(req, f, (tag)); \
        f++; \
    } while(0)

    SET_FIELD("method",      (taida_val)taida_str_new_copy(fields->method), TAIDA_TAG_STR);
    SET_FIELD("path",        (taida_val)taida_str_new_copy(path_part),       TAIDA_TAG_STR);
    SET_FIELD("query",       (taida_val)taida_str_new_copy(query_part),      TAIDA_TAG_STR);
    SET_FIELD("version",     version_pack,                                 TAIDA_TAG_PACK);
    SET_FIELD("headers",     hdr_list,                                     TAIDA_TAG_LIST);
    // NB6-26: Use TAIDA_TAG_PACK for Bytes (consistent with h1 path — Bytes use PACK tag in Native)
    SET_FIELD("body",        raw_bytes,                                    TAIDA_TAG_PACK);
    SET_FIELD("bodyOffset",  (taida_val)0,                                 TAIDA_TAG_INT);
    SET_FIELD("contentLength",(taida_val)(int64_t)body_len,                TAIDA_TAG_INT);
    // NB6-27: Retain raw_bytes before setting as second field to prevent double-free
    taida_retain(raw_bytes);
    SET_FIELD("raw",         raw_bytes,                                    TAIDA_TAG_PACK);
    SET_FIELD("remoteHost",  (taida_val)taida_str_new_copy(peer_host),       TAIDA_TAG_STR);
    SET_FIELD("remotePort",  (taida_val)(int64_t)peer_port,                TAIDA_TAG_INT);
    SET_FIELD("keepAlive",   (taida_val)1,                                 TAIDA_TAG_BOOL);
    // NB6-28: Add missing "chunked" field (HTTP/2 never uses chunked TE)
    SET_FIELD("chunked",     (taida_val)0,                                 TAIDA_TAG_BOOL);
    SET_FIELD("protocol",    (taida_val)taida_str_new_copy("h2"),            TAIDA_TAG_STR);
    #undef SET_FIELD
    return req;
}

// Append data to the CONTINUATION buffer (resizing as needed).
static int h2_continuation_append(H2Conn *conn, const unsigned char *data, uint32_t len) {
    if (len == 0) return 0;
    // Safety limit: prevent HPACK bomb / memory exhaustion
    if (conn->continuation_len + (size_t)len > H2_MAX_CONTINUATION_BUFFER_SIZE) return -1;
    if (conn->continuation_len + len > conn->continuation_cap) {
        size_t new_cap = conn->continuation_cap ? conn->continuation_cap * 2 : 4096;
        while (new_cap < conn->continuation_len + len) new_cap *= 2;
        if (new_cap > H2_MAX_CONTINUATION_BUFFER_SIZE) new_cap = H2_MAX_CONTINUATION_BUFFER_SIZE;
        unsigned char *nb = (unsigned char*)realloc(conn->continuation_buf, new_cap);
        if (!nb) return -1;
        conn->continuation_buf = nb;
        conn->continuation_cap = new_cap;
    }
    memcpy(conn->continuation_buf + conn->continuation_len, data, len);
    conn->continuation_len += len;
    return 0;
}

// ── taida_net_h2_serve_connection ─────────────────────────────────────────
// Serve one HTTP/2 connection on file descriptor `client_fd`.
// Returns after connection closes or max_requests reached.
// conn_send_window_ptr and stream_send_window_ptr are temporarily per-call.
static void taida_net_h2_serve_connection(int client_fd, H2ServeCtx *ctx) {
    // NB6-40: Heap-allocate H2Conn (~18KB) to avoid deep-stack overflow risk.
    H2Conn *connp = (H2Conn*)TAIDA_MALLOC(sizeof(H2Conn), "h2_conn");
    if (!connp) return;
    #define conn (*connp)
    h2_conn_init(&conn);

    // Validate connection preface
    {
        unsigned char preface[H2_CONNECTION_PREFACE_LEN];
        if (h2_read_exact(client_fd, preface, H2_CONNECTION_PREFACE_LEN) != H2_CONNECTION_PREFACE_LEN) {
            goto h2_conn_done;
        }
        if (memcmp(preface, H2_CONNECTION_PREFACE, H2_CONNECTION_PREFACE_LEN) != 0) {
            h2_send_goaway(client_fd, 0, H2_ERROR_PROTOCOL_ERROR, "invalid connection preface");
            goto h2_conn_done;
        }
    }

    // Send server SETTINGS
    if (h2_send_server_settings(client_fd, H2_DEFAULT_MAX_FRAME_SIZE,
                                 H2_DEFAULT_MAX_CONCURRENT_STREAMS) < 0) {
        goto h2_conn_done;
    }

    // Connection frame loop
    {
        int settings_ack_pending = 0;

        for (;;) {
            if (ctx->max_requests > 0 && *ctx->request_count >= ctx->max_requests) break;

            uint8_t frame_type, frame_flags;
            uint32_t frame_stream_id, payload_len;
            unsigned char *payload = NULL;

            int fr = h2_read_frame(client_fd, conn.local_max_frame_size,
                                    &frame_type, &frame_flags, &frame_stream_id,
                                    &payload, &payload_len);
            if (fr == 0) break; // clean close
            if (fr == -2) {
                // FRAME_SIZE_ERROR
                h2_send_goaway(client_fd, conn.last_peer_stream_id,
                               H2_ERROR_FRAME_SIZE_ERROR, "frame too large");
                conn.goaway_sent = 1;
                break;
            }
            if (fr < 0) break;

            // RFC 9113: during CONTINUATION sequence only CONTINUATION is allowed
            if (conn.continuation_stream_id != 0 && frame_type != H2_FRAME_CONTINUATION) {
                free(payload);
                h2_send_goaway(client_fd, conn.last_peer_stream_id,
                               H2_ERROR_PROTOCOL_ERROR, "expected CONTINUATION");
                conn.goaway_sent = 1;
                break;
            }

            // Accumulate SETTINGS ACK / PING tracking
            int is_ping_ack_needed = 0;
            unsigned char ping_data[8];
            if (frame_type == H2_FRAME_SETTINGS && !(frame_flags & H2_FLAG_ACK)) {
                settings_ack_pending = 1;
            }
            if (frame_type == H2_FRAME_PING && !(frame_flags & H2_FLAG_ACK) && payload_len == 8) {
                is_ping_ack_needed = 1;
                memcpy(ping_data, payload, 8);
            }

            // Dispatch by frame type
            int protocol_error = 0;
            int completed_stream_id = 0; // Non-zero if a request is ready

            switch (frame_type) {
                case H2_FRAME_SETTINGS: {
                    if (frame_stream_id != 0) { protocol_error = 1; break; }
                    if (frame_flags & H2_FLAG_ACK) {
                        if (payload_len != 0) { protocol_error = 1; break; }
                        break;
                    }
                    if (h2_process_settings(&conn, payload, payload_len) < 0) {
                        protocol_error = 1;
                    }
                    break;
                }

                case H2_FRAME_HEADERS: {
                    if (frame_stream_id == 0) { protocol_error = 1; break; }
                    if (frame_stream_id % 2 == 0) { protocol_error = 1; break; }
                    if (frame_stream_id <= conn.last_peer_stream_id) { protocol_error = 1; break; }
                    conn.last_peer_stream_id = frame_stream_id;

                    // Strip padding
                    uint32_t offset = 0, pad_len = 0;
                    if (frame_flags & H2_FLAG_PADDED) {
                        if (payload_len == 0) { protocol_error = 1; break; }
                        pad_len = payload[0];
                        offset = 1;
                    }
                    if (frame_flags & H2_FLAG_PRIORITY) offset += 5;
                    if (offset + pad_len > payload_len) { protocol_error = 1; break; }

                    const unsigned char *hdr_block = payload + offset;
                    uint32_t hdr_block_len = payload_len - offset - pad_len;

                    int end_headers = (frame_flags & H2_FLAG_END_HEADERS) != 0;
                    int end_stream  = (frame_flags & H2_FLAG_END_STREAM)  != 0;

                    // Create stream slot
                    H2Stream *s = h2_conn_new_stream(&conn, frame_stream_id);
                    if (!s) { protocol_error = 1; break; }

                    if (!end_headers) {
                        // Start CONTINUATION sequence
                        conn.continuation_stream_id = frame_stream_id;
                        conn.continuation_flags = frame_flags;
                        conn.continuation_len = 0;
                        if (h2_continuation_append(&conn, hdr_block, hdr_block_len) < 0) {
                            protocol_error = 1;
                        }
                        break;
                    }

                    // END_HEADERS: decode now
                    H2Header *hdrs = (H2Header*)TAIDA_MALLOC(sizeof(H2Header) * H2_MAX_HEADERS, "h2_headers");
                    if (!hdrs) { protocol_error = 1; break; }
                    int hdr_count = h2_hpack_decode_block(hdr_block, hdr_block_len,
                                                           &conn.decoder_dyn, hdrs, H2_MAX_HEADERS);
                    if (hdr_count < 0) {
                        free(hdrs);
                        h2_send_goaway(client_fd, conn.last_peer_stream_id,
                                       H2_ERROR_COMPRESSION_ERROR, "HPACK decode error");
                        conn.goaway_sent = 1;
                        free(payload);
                        goto h2_conn_done;
                    }
                    // Safety: enforce header list size limit (HPACK bomb protection)
                    if (h2_validate_header_list_size(hdrs, hdr_count) < 0) {
                        free(hdrs);
                        h2_send_goaway(client_fd, conn.last_peer_stream_id,
                                       H2_ERROR_INTERNAL_ERROR, "decoded header list too large");
                        conn.goaway_sent = 1;
                        free(payload);
                        goto h2_conn_done;
                    }
                    s->request_headers = hdrs;
                    s->request_header_count = hdr_count;
                    s->state = H2_STREAM_HALF_CLOSED_REMOTE;

                    if (end_stream) {
                        completed_stream_id = (int)frame_stream_id;
                    }
                    break;
                }

                case H2_FRAME_DATA: {
                    if (frame_stream_id == 0) { protocol_error = 1; break; }
                    H2Stream *s = h2_conn_find_stream(&conn, frame_stream_id);
                    if (!s) { h2_send_rst_stream(client_fd, frame_stream_id, H2_ERROR_STREAM_CLOSED); break; }

                    // Strip padding
                    uint32_t offset = 0, pad_len = 0;
                    if (frame_flags & H2_FLAG_PADDED) {
                        if (payload_len == 0) { protocol_error = 1; break; }
                        pad_len = payload[0];
                        offset = 1;
                    }
                    if (offset + pad_len > payload_len) { protocol_error = 1; break; }

                    int64_t data_len = (int64_t)(payload_len); // includes padding in window
                    // Flow control enforcement
                    if (data_len > conn.conn_recv_window) {
                        h2_send_goaway(client_fd, conn.last_peer_stream_id,
                                       H2_ERROR_FLOW_CONTROL_ERROR, "connection recv window exceeded");
                        conn.goaway_sent = 1;
                        free(payload);
                        goto h2_conn_done;
                    }
                    if (data_len > s->recv_window) {
                        // Stream-level violation: RST_STREAM + close stream + continue
                        // (matches Interpreter: H2Error::Stream → send_rst_stream → continue)
                        h2_send_rst_stream(client_fd, frame_stream_id, H2_ERROR_FLOW_CONTROL_ERROR);
                        s->state = H2_STREAM_CLOSED;
                        h2_conn_remove_closed_streams(&conn);
                        free(payload);
                        continue;
                    }
                    conn.conn_recv_window -= data_len;
                    s->recv_window -= data_len;

                    const unsigned char *data = payload + offset;
                    uint32_t data_bytes = payload_len - offset - pad_len;
                    // Accumulate body
                    if (s->request_body_len + data_bytes > s->request_body_cap) {
                        size_t new_cap = s->request_body_cap ? s->request_body_cap * 2 : 4096;
                        while (new_cap < s->request_body_len + data_bytes) new_cap *= 2;
                        unsigned char *nb = (unsigned char*)realloc(s->request_body, new_cap);
                        if (!nb) { protocol_error = 1; break; }
                        s->request_body = nb;
                        s->request_body_cap = new_cap;
                    }
                    memcpy(s->request_body + s->request_body_len, data, data_bytes);
                    s->request_body_len += data_bytes;

                    if (frame_flags & H2_FLAG_END_STREAM) {
                        completed_stream_id = (int)frame_stream_id;
                    }
                    break;
                }

                case H2_FRAME_WINDOW_UPDATE: {
                    if (payload_len != 4) { protocol_error = 1; break; }
                    uint32_t increment = (((uint32_t)(payload[0] & 0x7F)) << 24) |
                                         ((uint32_t)payload[1] << 16) |
                                         ((uint32_t)payload[2] << 8)  |
                                          (uint32_t)payload[3];
                    if (increment == 0) { protocol_error = 1; break; }
                    if (frame_stream_id == 0) {
                        // RFC 9113 Section 6.9.1: window MUST NOT exceed 2^31-1
                        int64_t new_window = conn.conn_send_window + (int64_t)increment;
                        if (new_window > H2_MAX_FLOW_CONTROL_WINDOW) {
                            free(payload);
                            h2_send_goaway(client_fd, conn.last_peer_stream_id,
                                           H2_ERROR_FLOW_CONTROL_ERROR,
                                           "WINDOW_UPDATE would overflow connection window");
                            conn.goaway_sent = 1;
                            goto h2_conn_done;
                        }
                        conn.conn_send_window = new_window;
                    } else {
                        H2Stream *s = h2_conn_find_stream(&conn, frame_stream_id);
                        if (s) {
                            int64_t new_window = s->send_window + (int64_t)increment;
                            if (new_window > H2_MAX_FLOW_CONTROL_WINDOW) {
                                h2_send_rst_stream(client_fd, frame_stream_id,
                                                   H2_ERROR_FLOW_CONTROL_ERROR);
                                s->state = H2_STREAM_CLOSED;
                            } else {
                                s->send_window = new_window;
                            }
                        }
                    }
                    break;
                }

                case H2_FRAME_PING: {
                    if (frame_stream_id != 0) { protocol_error = 1; break; }
                    if (payload_len != 8) { protocol_error = 1; break; }
                    // ACK handled below
                    break;
                }

                case H2_FRAME_GOAWAY:
                    // Client is shutting down
                    free(payload);
                    goto h2_conn_done;

                case H2_FRAME_RST_STREAM: {
                    if (frame_stream_id == 0) { protocol_error = 1; break; }
                    // NB6-31: RFC 9113 Section 6.4 — RST_STREAM payload MUST be exactly 4 bytes
                    if (payload_len != 4) { protocol_error = 1; break; }
                    H2Stream *s = h2_conn_find_stream(&conn, frame_stream_id);
                    if (s) s->state = H2_STREAM_CLOSED;
                    break;
                }

                case H2_FRAME_PRIORITY: {
                    if (payload_len != 5) { protocol_error = 1; break; }
                    break; // advisory, ignored
                }

                case H2_FRAME_PUSH_PROMISE: {
                    // Client sending PUSH_PROMISE is a protocol error
                    protocol_error = 1;
                    break;
                }

                case H2_FRAME_CONTINUATION: {
                    if (conn.continuation_stream_id == 0) { protocol_error = 1; break; }
                    if (frame_stream_id != conn.continuation_stream_id) { protocol_error = 1; break; }

                    if (h2_continuation_append(&conn, payload, payload_len) < 0) {
                        protocol_error = 1; break;
                    }

                    int end_headers = (frame_flags & H2_FLAG_END_HEADERS) != 0;
                    if (!end_headers) break; // more CONTINUATION expected

                    // END_HEADERS: decode complete header block
                    uint32_t sid = conn.continuation_stream_id;
                    uint8_t orig_flags = conn.continuation_flags;
                    int end_stream = (orig_flags & H2_FLAG_END_STREAM) != 0;

                    H2Stream *s = h2_conn_find_stream(&conn, sid);
                    if (!s) {
                        // Create if not found (shouldn't happen for valid flow)
                        s = h2_conn_new_stream(&conn, sid);
                        if (!s) { protocol_error = 1; break; }
                    }

                    H2Header *hdrs = (H2Header*)TAIDA_MALLOC(sizeof(H2Header) * H2_MAX_HEADERS, "h2_cont_headers");
                    if (!hdrs) { protocol_error = 1; break; }
                    int hdr_count = h2_hpack_decode_block(conn.continuation_buf,
                                                           conn.continuation_len,
                                                           &conn.decoder_dyn, hdrs, H2_MAX_HEADERS);
                    conn.continuation_stream_id = 0;
                    conn.continuation_flags = 0;
                    conn.continuation_len = 0;

                    if (hdr_count < 0) {
                        free(hdrs);
                        h2_send_goaway(client_fd, conn.last_peer_stream_id,
                                       H2_ERROR_COMPRESSION_ERROR, "HPACK decode error in CONTINUATION");
                        conn.goaway_sent = 1;
                        free(payload);
                        goto h2_conn_done;
                    }
                    // Safety: enforce header list size limit (HPACK bomb protection)
                    if (h2_validate_header_list_size(hdrs, hdr_count) < 0) {
                        free(hdrs);
                        h2_send_goaway(client_fd, conn.last_peer_stream_id,
                                       H2_ERROR_INTERNAL_ERROR, "decoded header list too large in CONTINUATION");
                        conn.goaway_sent = 1;
                        free(payload);
                        goto h2_conn_done;
                    }
                    free(s->request_headers);
                    s->request_headers = hdrs;
                    s->request_header_count = hdr_count;
                    s->state = H2_STREAM_HALF_CLOSED_REMOTE;

                    if (end_stream) completed_stream_id = (int)sid;
                    break;
                }

                default:
                    break; // Unknown frame types ignored (RFC 9113 Section 4.1)
            }

            free(payload);

            if (protocol_error) {
                h2_send_goaway(client_fd, conn.last_peer_stream_id,
                               H2_ERROR_PROTOCOL_ERROR, "protocol error");
                conn.goaway_sent = 1;
                goto h2_conn_done;
            }

            // Send SETTINGS ACK if we processed a SETTINGS frame
            if (settings_ack_pending) {
                if (h2_send_settings_ack(client_fd) < 0) goto h2_conn_done;
                settings_ack_pending = 0;
            }

            // Send PING ACK if needed
            if (is_ping_ack_needed) {
                h2_send_ping_ack(client_fd, ping_data, 8);
            }

            // Dispatch completed request
            if (completed_stream_id > 0) {
                H2Stream *s = h2_conn_find_stream(&conn, (uint32_t)completed_stream_id);
                if (!s) continue;

                // Replenish receive window
                if (s->request_body_len > 0) {
                    uint32_t inc = (uint32_t)s->request_body_len;
                    h2_send_window_update(client_fd, 0, inc);
                    h2_send_window_update(client_fd, (uint32_t)completed_stream_id, inc);
                    conn.conn_recv_window += inc;
                    s->recv_window += inc;
                }

                // Extract request fields
                H2RequestFields req_fields;
                h2_extract_request_fields(s->request_headers, s->request_header_count, &req_fields);

                if (!req_fields.ok) {
                    h2_send_rst_stream(client_fd, (uint32_t)completed_stream_id, H2_ERROR_PROTOCOL_ERROR);
                    s->state = H2_STREAM_CLOSED;
                    h2_conn_remove_closed_streams(&conn);
                    continue;
                }

                // Build request pack and call handler
                taida_val req_pack = h2_build_request_pack(
                    &req_fields,
                    s->request_body, s->request_body_len,
                    ctx->peer_host, ctx->peer_port
                );
                free(req_fields.regular_headers);

                taida_val response = h2_dispatch_request(ctx, req_pack);
                (*ctx->request_count)++;

                // Extract and send response
                H2ResponseFields resp;
                h2_extract_response_fields(response, &resp);

                int no_body = (resp.status >= 100 && resp.status < 200) ||
                              resp.status == 204 || resp.status == 205 || resp.status == 304;
                int has_body = resp.ok && resp.body && resp.body_len > 0 && !no_body;

                if (!has_body) {
                    /* D28B-025: RFC 9113 §8.1.1 + RFC 9110 §6.4 forbid
                     * content-length / transfer-encoding on no-body
                     * responses (1xx / 204 / 205 / 304). Strip them
                     * before HPACK encode if a user handler set them.
                     * h1 path has the same protection at no_body status
                     * codes; h2 was missing the symmetric guard prior
                     * to D28B-025. Use a filtered copy so the original
                     * `resp.headers` (owned by the response pack) is
                     * untouched and freed correctly by the caller. */
                    H2Header *send_hdrs = resp.headers;
                    int send_count = resp.header_count;
                    H2Header *filtered = NULL;
                    if (no_body && resp.header_count > 0) {
                        int needs_strip = 0;
                        for (int hi = 0; hi < resp.header_count; hi++) {
                            if (strcasecmp(resp.headers[hi].name, "content-length") == 0 ||
                                strcasecmp(resp.headers[hi].name, "transfer-encoding") == 0) {
                                needs_strip = 1; break;
                            }
                        }
                        if (needs_strip) {
                            filtered = (H2Header*)TAIDA_MALLOC(
                                sizeof(H2Header) * (size_t)resp.header_count,
                                "h2_resp_hdrs_strip"
                            );
                            if (filtered) {
                                int fc = 0;
                                for (int hi = 0; hi < resp.header_count; hi++) {
                                    if (strcasecmp(resp.headers[hi].name, "content-length") != 0 &&
                                        strcasecmp(resp.headers[hi].name, "transfer-encoding") != 0) {
                                        filtered[fc++] = resp.headers[hi];
                                    }
                                }
                                send_hdrs = filtered;
                                send_count = fc;
                            }
                            /* On allocation failure, fall back to the
                             * original headers — the protocol-correct
                             * outcome is still better than dropping the
                             * request, and OOM is already a degraded
                             * mode. */
                        }
                    }
                    h2_send_response_headers(
                        client_fd, &conn.encoder_dyn,
                        (uint32_t)completed_stream_id, resp.status,
                        send_hdrs, send_count,
                        1 /*end_stream*/, conn.peer_max_frame_size
                    );
                    if (filtered) free(filtered);
                } else {
                    // Add content-length if not present
                    int has_cl = 0;
                    for (int hi = 0; hi < resp.header_count; hi++) {
                        if (strcasecmp(resp.headers[hi].name, "content-length") == 0) {
                            has_cl = 1; break;
                        }
                    }
                    H2Header *all_hdrs = resp.headers;
                    int all_count = resp.header_count;
                    H2Header cl_hdr;
                    if (!has_cl) {
                        // Allocate extended header array
                        all_hdrs = (H2Header*)TAIDA_MALLOC(sizeof(H2Header) * (size_t)(resp.header_count + 1), "h2_resp_hdrs_cl");
                        if (all_hdrs) {
                            memcpy(all_hdrs, resp.headers, sizeof(H2Header) * (size_t)resp.header_count);
                            snprintf(cl_hdr.name, sizeof(cl_hdr.name), "content-length");
                            snprintf(cl_hdr.value, sizeof(cl_hdr.value), "%zu", resp.body_len);
                            all_hdrs[resp.header_count] = cl_hdr;
                            all_count = resp.header_count + 1;
                        } else {
                            all_hdrs = resp.headers;
                            all_count = resp.header_count;
                        }
                    }
                    h2_send_response_headers(
                        client_fd, &conn.encoder_dyn,
                        (uint32_t)completed_stream_id, resp.status,
                        all_hdrs, all_count,
                        0 /*no end_stream*/, conn.peer_max_frame_size
                    );
                    if (all_hdrs != resp.headers) free(all_hdrs);

                    int64_t stream_sw = s->send_window;
                    int64_t data_sent = h2_send_response_data(
                        client_fd, (uint32_t)completed_stream_id,
                        resp.body, resp.body_len, 1 /*end_stream*/,
                        conn.peer_max_frame_size,
                        &conn.conn_send_window, &stream_sw
                    );
                    s->send_window = stream_sw;
                    if (data_sent < 0) {
                        // Flow control exhausted — send RST_STREAM and continue
                        h2_send_rst_stream(client_fd, (uint32_t)completed_stream_id,
                                           H2_ERROR_FLOW_CONTROL_ERROR);
                    }
                }

                h2_response_fields_free(&resp);

                /* D28B-002 (Round 2 wG): release request + response packs.
                 *
                 * h2_extract_response_fields() above copied every field
                 * we still need into malloc-backed buffers in `resp`,
                 * and h2_response_fields_free(&resp) just released
                 * those copies. The taida_val req_pack and response
                 * were leaked at the refcount level pre-wG -- their
                 * arena-backed slots stayed reachable forever via the
                 * orphaned refcount, even though no code dereferenced
                 * them after this point. Drive the refcounts to 0 so
                 * the arena reset below has the same safety footing
                 * as the h1 worker's per-iteration reset. */
                taida_release(req_pack);
                taida_release(response);

                s->state = H2_STREAM_CLOSED;
                h2_conn_remove_closed_streams(&conn);

                /* D28B-002 (Round 2 wG): per-stream arena boundary.
                 *
                 * Every per-request taida_val (the 14-field request
                 * pack, header 2-field packs, body Bytes, str_new_copy
                 * strings for method / path / query / authority / peer
                 * host / protocol, plus the handler-returned response
                 * pack) has had its refcount driven to 0 just above.
                 * Their arena-backed slots are logically dead but the
                 * bump arena offset has not rewound -- this is the h2
                 * twin of D28B-012 (4 GB plateau / 4.7 GB/h drift on
                 * the h1 worker), measured at ~2.5 MiB / 1k req on
                 * the h2 path before this fix.
                 *
                 * Safety: H2Conn (heap-malloc'd at L5635), the
                 * encoder/decoder HPACK dyn tables (strdup-backed),
                 * surviving H2Stream entries (request_headers /
                 * request_body all TAIDA_MALLOC'd), and the handler
                 * closure (lives on this same main thread but is
                 * refcount-tracked, not arena-bound) are all
                 * unaffected by an arena reset. The connection's
                 * scratch buffers (continuation_buf etc.) are
                 * realloc-backed.
                 *
                 * Single-thread caveat: taida_net_h2_serve runs on
                 * the application's main thread (no worker pool, no
                 * pthread_create here), so this resets the same
                 * __thread arena that the h1 worker thread version
                 * resets. wF (D28B-012 fix) installed the helper as
                 * `static` in core.c so this file already has access
                 * to it through the build's translation-unit
                 * concatenation (mod.rs F0_LEN..F6_LEN). */
                taida_arena_request_reset();
            }
        }
    }

h2_conn_done:
    if (!conn.goaway_sent) {
        h2_send_goaway(client_fd, conn.last_peer_stream_id, H2_ERROR_NO_ERROR, "");
    }
    h2_conn_free(&conn);
    #undef conn
    free(connp);

    /* D28B-002 (Round 2 wG): connection-boundary arena reset.
     *
     * Catches the early-exit paths inside the frame loop (preface
     * mismatch, h2_send_server_settings failure, frame read errors,
     * GOAWAY exits, HPACK decode errors, h2_continuation_append
     * overflow, oversized header list) where the per-stream reset
     * could not fire because the loop bailed before reaching the
     * `completed_stream_id` block. Idempotent if already drained
     * (the freelist drain loops exit on count == 0 and the
     * keep-chunk[0]-rewind path is a no-op when chunk[0] is at
     * offset 0). */
    taida_arena_request_reset();
}

typedef struct { int64_t requests; } H2ServeResult;

// ── taida_net_h2_serve ─────────────────────────────────────────────────────
// Full HTTP/2 server loop: bind → accept → TLS handshake → ALPN check → serve.
// max_requests=0 means unlimited. Returns request count and connection count.
static H2ServeResult taida_net_h2_serve(int port, taida_val handler, int handler_arity,
                                         int64_t max_requests, int64_t timeout_ms,
                                         const char *cert_path, const char *key_path) {
    H2ServeResult fail_result = {-1};

    // Load OpenSSL (required for h2 — h2c is out of scope)
    if (!taida_ossl_load()) {
        return fail_result;
    }

    // Create TLS context with ALPN h2 / http/1.1
    char errbuf[512];
    OSSL_SSL_CTX *ssl_ctx = taida_tls_create_ctx_h2(cert_path, key_path, errbuf, sizeof(errbuf));
    if (!ssl_ctx) {
        return fail_result;
    }

    // Bind to 127.0.0.1:port
    int sockfd = socket(AF_INET, SOCK_STREAM, 0);
    if (sockfd < 0) { taida_ossl.SSL_CTX_free(ssl_ctx); return fail_result; }
    int opt = 1;
    setsockopt(sockfd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    inet_pton(AF_INET, "127.0.0.1", &addr.sin_addr);
    addr.sin_port = htons((unsigned short)port);
    if (bind(sockfd, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        close(sockfd); taida_ossl.SSL_CTX_free(ssl_ctx); return fail_result;
    }
    if (listen(sockfd, 128) < 0) {
        close(sockfd); taida_ossl.SSL_CTX_free(ssl_ctx); return fail_result;
    }

    // C27B-014: opt-in port announcement (h2 path). Same env var name
    // and surface format as h1 / interpreter / JS. Default OFF.
    {
        const char *announce = getenv("TAIDA_NET_ANNOUNCE_PORT");
        if (announce && announce[0] == '1' && announce[1] == '\0') {
            struct sockaddr_in bound_addr;
            socklen_t bound_len = sizeof(bound_addr);
            if (getsockname(sockfd, (struct sockaddr*)&bound_addr, &bound_len) == 0) {
                printf("listening on 127.0.0.1:%u\n", (unsigned int)ntohs(bound_addr.sin_port));
                fflush(stdout);
            }
        }
    }

    int64_t request_count = 0;
    int64_t connection_count = 0;
    signal(SIGPIPE, SIG_IGN);

    while (max_requests == 0 || request_count < max_requests) {
        // Accept with timeout so we can re-check request count
        struct timeval tv;
        tv.tv_sec = 0;
        tv.tv_usec = 100000; // 100ms
        setsockopt(sockfd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));

        struct sockaddr_in peer_addr;
        socklen_t peer_len = sizeof(peer_addr);
        int client_fd = accept(sockfd, (struct sockaddr*)&peer_addr, &peer_len);
        if (client_fd < 0) {
            if (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) continue;
            break;
        }

        // TLS handshake
        {
            struct timeval to;
            to.tv_sec = (timeout_ms > 0) ? timeout_ms / 1000 : 30;
            to.tv_usec = (timeout_ms > 0) ? (timeout_ms % 1000) * 1000 : 0;
            setsockopt(client_fd, SOL_SOCKET, SO_RCVTIMEO, &to, sizeof(to));
            setsockopt(client_fd, SOL_SOCKET, SO_SNDTIMEO, &to, sizeof(to));
        }

        OSSL_SSL *ssl = taida_tls_handshake(ssl_ctx, client_fd);
        if (!ssl) { close(client_fd); continue; }

        // ALPN check: only proceed if "h2" was negotiated
        int h2_negotiated = 0;
        if (taida_ossl.SSL_get0_alpn_selected) {
            const unsigned char *alpn_data = NULL;
            unsigned int alpn_len = 0;
            taida_ossl.SSL_get0_alpn_selected(ssl, &alpn_data, &alpn_len);
            if (alpn_data && alpn_len == 2 &&
                alpn_data[0] == 'h' && alpn_data[1] == '2') {
                h2_negotiated = 1;
            }
        } else {
            // ALPN API not available — assume h2 (only h2 clients should connect here)
            h2_negotiated = 1;
        }

        if (!h2_negotiated) {
            // No silent fallback: close connection per design policy
            taida_tls_shutdown_free(ssl);
            close(client_fd);
            continue;
        }

        connection_count++;
        // NB6-47: emit connection count to stderr (side channel for benchmarks).
        // This keeps the public result pack contract clean (@(requests: Int) only).
        fprintf(stderr, "[h2-conn] %lld\n", (long long)connection_count);

        // Set TLS for this connection's I/O
        tl_ssl = ssl;

        // Get peer info
        char peer_host[64];
        int peer_port_val = ntohs(peer_addr.sin_port);
        if (!inet_ntop(AF_INET, &peer_addr.sin_addr, peer_host, sizeof(peer_host))) {
            snprintf(peer_host, sizeof(peer_host), "127.0.0.1");
        }

        H2ServeCtx serve_ctx;
        serve_ctx.handler = handler;
        serve_ctx.handler_arity = handler_arity;
        serve_ctx.request_count = &request_count;
        serve_ctx.max_requests = max_requests;
        snprintf(serve_ctx.peer_host, sizeof(serve_ctx.peer_host), "%s", peer_host);
        serve_ctx.peer_port = peer_port_val;

        taida_net_h2_serve_connection(client_fd, &serve_ctx);

        // TLS shutdown — bidirectional: first call sends close-notify,
        // second call waits for peer's close-notify (or EAGAIN/EWOULDBLOCK).
        // This ensures all buffered response data reaches the client before
        // the TCP connection is torn down (avoids RST truncating the response).
        if (ssl) {
            int sd1 = taida_ossl.SSL_shutdown(ssl);
            if (sd1 == 0) {
                // First shutdown sent, wait for peer. Drain incoming bytes.
                unsigned char drain_buf[256];
                int drain_attempts = 0;
                while (drain_attempts++ < 20) {
                    int r = taida_ossl.SSL_read(ssl, drain_buf, (int)sizeof(drain_buf));
                    if (r <= 0) break;
                }
                taida_ossl.SSL_shutdown(ssl); // second call — receive peer's close-notify
            }
            taida_ossl.SSL_free(ssl);
        }
        tl_ssl = NULL;
        // TCP half-close + brief drain to ensure kernel flushes send buffer.
        shutdown(client_fd, SHUT_WR);
        {
            unsigned char tcp_drain[256];
            struct timeval tv2 = {0, 50000}; // 50ms
            setsockopt(client_fd, SOL_SOCKET, SO_RCVTIMEO, &tv2, sizeof(tv2));
            int d;
            while ((d = (int)recv(client_fd, tcp_drain, sizeof(tcp_drain), 0)) > 0) {}
        }
        close(client_fd);
    }

    close(sockfd);
    taida_ossl.SSL_CTX_free(ssl_ctx);
    H2ServeResult ok_result = {request_count};
    return ok_result;
}

