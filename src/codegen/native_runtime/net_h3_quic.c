// ── H3/QPACK constants (NET7-2a/2b) ──────────────────────────────────────
// HTTP/3 frame types (RFC 9114 Section 7.2)
#define H3_FRAME_DATA           0x00
#define H3_FRAME_HEADERS        0x01
#define H3_FRAME_CANCEL_PUSH    0x03
#define H3_FRAME_SETTINGS       0x04
#define H3_FRAME_PUSH_PROMISE   0x05
#define H3_FRAME_GOAWAY         0x07
#define H3_FRAME_MAX_PUSH_ID    0x0D

// H3 error codes (RFC 9114 Section 8.1)
#define H3_ERROR_NO_ERROR                  0x0100
#define H3_ERROR_GENERAL_PROTOCOL_ERROR    0x0101
#define H3_ERROR_INTERNAL_ERROR            0x0102
#define H3_ERROR_STREAM_CREATION_ERROR     0x0103
#define H3_ERROR_CLOSED_CRITICAL_STREAM    0x0104
#define H3_ERROR_FRAME_UNEXPECTED          0x0105
#define H3_ERROR_FRAME_ERROR               0x0106
#define H3_ERROR_EXCESSIVE_LOAD            0x0107
#define H3_ERROR_ID_ERROR                  0x0108
#define H3_ERROR_SETTINGS_ERROR            0x0109
#define H3_ERROR_MISSING_SETTINGS          0x010A
#define H3_ERROR_REQUEST_REJECTED          0x010B
#define H3_ERROR_REQUEST_CANCELLED         0x010C
#define H3_ERROR_REQUEST_INCOMPLETE        0x010D
#define H3_ERROR_MESSAGE_ERROR             0x010E
#define H3_ERROR_CONNECT_ERROR             0x010F
#define H3_ERROR_VERSION_FALLBACK          0x0110

// H3 settings identifiers (RFC 9114 Section 7.2.4.1)
#define H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY   0x01
#define H3_SETTINGS_MAX_FIELD_SECTION_SIZE     0x06
#define H3_SETTINGS_QPACK_BLOCKED_STREAMS      0x07

// H3 stream types (RFC 9114 Section 6.2)
#define H3_STREAM_TYPE_CONTROL  0x00
#define H3_STREAM_TYPE_PUSH     0x01
#define H3_STREAM_TYPE_QPACK_ENCODER 0x02
#define H3_STREAM_TYPE_QPACK_DECODER 0x03

// H3 defaults
#define H3_DEFAULT_MAX_FIELD_SECTION_SIZE (64 * 1024)
#define H3_MAX_HEADERS 128
#define H3_MAX_STREAMS 256

// ── QPACK static table (RFC 9204 Appendix A) ──────────────────────────────
// QPACK uses a different static table than HPACK. 99 entries (indices 0-98).

typedef struct {
    const char *name;
    const char *value;
} H3QpackStaticEntry;

// H3 QPACK Static Table (RFC 9204 Appendix A).
// NB7-36: Entries 0-98 fully match RFC. Entry 99 (":path" "/index.html") is
// intentionally omitted — typical web apps rarely serve "/index.html" as a static
// path. Parity: static table indices must be identical on both backends.
static const H3QpackStaticEntry H3_QPACK_STATIC_TABLE[] = {
    { ":authority", "" },                         // 0
    { ":path", "/" },                             // 1
    { "age", "0" },                               // 2
    { "content-disposition", "" },                // 3
    { "content-length", "0" },                    // 4
    { "cookie", "" },                             // 5
    { "date", "" },                               // 6
    { "etag", "" },                               // 7
    { "if-modified-since", "" },                  // 8
    { "if-none-match", "" },                      // 9
    { "last-modified", "" },                      // 10
    { "link", "" },                               // 11
    { "location", "" },                           // 12
    { "referer", "" },                            // 13
    { "set-cookie", "" },                         // 14
    { ":method", "CONNECT" },                     // 15
    { ":method", "DELETE" },                      // 16
    { ":method", "GET" },                         // 17
    { ":method", "HEAD" },                        // 18
    { ":method", "OPTIONS" },                     // 19
    { ":method", "POST" },                        // 20
    { ":method", "PUT" },                         // 21
    { ":scheme", "http" },                        // 22
    { ":scheme", "https" },                       // 23
    { ":status", "103" },                         // 24
    { ":status", "200" },                         // 25
    { ":status", "304" },                         // 26
    { ":status", "404" },                         // 27
    { ":status", "503" },                         // 28
    { "accept", "*/*" },                          // 29
    { "accept", "application/dns-message" },      // 30
    { "accept-encoding", "gzip, deflate, br" },   // 31
    { "accept-ranges", "bytes" },                 // 32
    { "access-control-allow-headers", "cache-control" }, // 33
    { "access-control-allow-headers", "content-type" },  // 34
    { "access-control-allow-origin", "*" },       // 35
    { "cache-control", "max-age=0" },             // 36
    { "cache-control", "max-age=2592000" },       // 37
    { "cache-control", "max-age=604800" },        // 38
    { "cache-control", "no-cache" },              // 39
    { "cache-control", "no-store" },              // 40
    { "cache-control", "public, max-age=31536000" }, // 41
    { "content-encoding", "br" },                 // 42
    { "content-encoding", "gzip" },               // 43
    { "content-type", "application/dns-message" }, // 44
    { "content-type", "application/javascript" },  // 45
    { "content-type", "application/json" },        // 46
    { "content-type", "application/x-www-form-urlencoded" }, // 47
    { "content-type", "image/gif" },               // 48
    { "content-type", "image/jpeg" },              // 49
    { "content-type", "image/png" },               // 50
    { "content-type", "text/css" },                // 51
    { "content-type", "text/html; charset=utf-8" }, // 52
    { "content-type", "text/plain" },              // 53
    { "content-type", "text/plain;charset=utf-8" }, // 54
    { "range", "bytes=0-" },                       // 55
    { "strict-transport-security", "max-age=31536000" }, // 56
    { "strict-transport-security", "max-age=31536000; includesubdomains" }, // 57
    { "strict-transport-security", "max-age=31536000; includesubdomains; preload" }, // 58
    { "vary", "accept-encoding" },                 // 59
    { "vary", "origin" },                          // 60
    { "x-content-type-options", "nosniff" },       // 61
    { "x-xss-protection", "1; mode=block" },       // 62
    { ":status", "100" },                          // 63
    { ":status", "204" },                          // 64
    { ":status", "206" },                          // 65
    { ":status", "302" },                          // 66
    { ":status", "400" },                          // 67
    { ":status", "403" },                          // 68
    { ":status", "421" },                          // 69
    { ":status", "425" },                          // 70
    { ":status", "500" },                          // 71
    { "accept-language", "" },                     // 72
    { "access-control-allow-credentials", "FALSE" }, // 73
    { "access-control-allow-credentials", "TRUE" },  // 74
    { "access-control-allow-headers", "*" },       // 75
    { "access-control-allow-methods", "get" },     // 76
    { "access-control-allow-methods", "get, post, options" }, // 77
    { "access-control-allow-methods", "options" },  // 78
    { "access-control-expose-headers", "content-length" }, // 79
    { "access-control-request-headers", "content-type" },  // 80
    { "access-control-request-method", "get" },    // 81
    { "access-control-request-method", "post" },   // 82
    { "alt-svc", "clear" },                        // 83
    { "authorization", "" },                       // 84
    { "content-security-policy", "script-src 'none'; object-src 'none'; base-uri 'none'" }, // 85
    { "early-data", "1" },                         // 86
    { "expect-ct", "" },                           // 87
    { "forwarded", "" },                           // 88
    { "if-range", "" },                            // 89
    { "origin", "" },                              // 90
    { "purpose", "prefetch" },                     // 91
    { "server", "" },                              // 92
    { "timing-allow-origin", "*" },                // 93
    { "upgrade-insecure-requests", "1" },          // 94
    { "user-agent", "" },                          // 95
    { "x-forwarded-for", "" },                     // 96
    { "x-frame-options", "deny" },                 // 97
    { "x-frame-options", "sameorigin" },           // 98
};
#define H3_QPACK_STATIC_TABLE_LEN (sizeof(H3_QPACK_STATIC_TABLE) / sizeof(H3_QPACK_STATIC_TABLE[0]))

// ── QPACK Dynamic Table (RFC 9204 Section 4.3) (NET7-10d) ────────────────
// Parity with Interpreter's H3DynamicTable. Uses a bounded array (oldest
// first, newest last) with absolute indices for eviction semantics.

#define H3_DYNAMIC_TABLE_MAX_ENTRIES 256

typedef struct {
    char name[128];
    char value[256];
    uint64_t index;   // absolute index
    int active;       // 0 = free, 1 = occupied
} H3DynamicTableEntry;

typedef struct {
    H3DynamicTableEntry entries[H3_DYNAMIC_TABLE_MAX_ENTRIES];
    size_t current_size;       // sum of name.len + value.len + 32 per entry
    size_t max_capacity;       // maximum capacity in bytes
    uint64_t next_absolute_index;
    uint64_t largest_ref;      // TotalInsertions - 1
    uint64_t total_inserted;   // never decreases, even on eviction
} H3DynamicTable;

/// Initialize a dynamic table with the given byte capacity.
static void h3_dynamic_table_init(H3DynamicTable *dt, size_t capacity) {
    dt->current_size = 0;
    dt->max_capacity = capacity;
    dt->next_absolute_index = 0;
    dt->largest_ref = 0;
    dt->total_inserted = 0;
    for (int i = 0; i < H3_DYNAMIC_TABLE_MAX_ENTRIES; i++) {
        dt->entries[i].active = 0;
        dt->entries[i].index = 0;
        dt->entries[i].name[0] = '\0';
        dt->entries[i].value[0] = '\0';
    }
}

/// Number of active entries.
static size_t h3_dt_len(const H3DynamicTable *dt) {
    size_t count = 0;
    for (int i = 0; i < H3_DYNAMIC_TABLE_MAX_ENTRIES; i++) {
        if (dt->entries[i].active) count++;
    }
    return count;
}

static int h3_dt_is_empty(const H3DynamicTable *dt) {
    return h3_dt_len(dt) == 0;
}

static size_t h3_dt_current_size(const H3DynamicTable *dt) {
    return dt->current_size;
}

static size_t h3_dt_capacity(const H3DynamicTable *dt) {
    return dt->max_capacity;
}

static uint64_t h3_dt_largest_ref(const H3DynamicTable *dt) {
    return dt->largest_ref;
}

static uint64_t h3_dt_total_inserted(const H3DynamicTable *dt) {
    return dt->total_inserted;
}

/// NB7-112 fix: Evict oldest active entries until current_size <= new_capacity.
/// Previously broke on first inactive slot, which caused sparse-table
/// under-eviction: if earlier shrink left a hole at slot 0, the loop
/// would immediately hit that inactive slot and break, failing to evict
/// later active entries.
static void h3_dt_evict_to_capacity(H3DynamicTable *dt, size_t new_capacity) {
    int progress;
    do {
        progress = 0;
        for (int i = 0; i < H3_DYNAMIC_TABLE_MAX_ENTRIES; i++) {
            if (dt->entries[i].active && dt->current_size > new_capacity) {
                size_t entry_size = strlen(dt->entries[i].name) + strlen(dt->entries[i].value) + 32;
                dt->current_size = dt->current_size > entry_size ? dt->current_size - entry_size : 0;
                dt->entries[i].active = 0;
                dt->entries[i].name[0] = '\0';
                dt->entries[i].value[0] = '\0';
                progress = 1;
            }
        }
    } while (progress && dt->current_size > new_capacity);
    dt->max_capacity = new_capacity;
}

/// Insert an entry, evicting oldest entries if needed.
/// Returns 1 on success, 0 if entry alone exceeds capacity.
static int h3_dt_insert(H3DynamicTable *dt, const char *name, const char *value) {
    size_t nlen = strlen(name);
    size_t vlen = strlen(value);
    size_t entry_size = nlen + vlen + 32;

    if (entry_size > dt->max_capacity) return 0;

    // Evict to make room
    while (dt->current_size + entry_size > dt->max_capacity) {
        int found = 0;
        for (int i = 0; i < H3_DYNAMIC_TABLE_MAX_ENTRIES; i++) {
            if (dt->entries[i].active) {
                size_t ev_sz = strlen(dt->entries[i].name) + strlen(dt->entries[i].value) + 32;
                dt->current_size = dt->current_size > ev_sz ? dt->current_size - ev_sz : 0;
                dt->entries[i].active = 0;
                dt->entries[i].name[0] = '\0';
                dt->entries[i].value[0] = '\0';
                found = 1;
                break;
            }
        }
        if (!found) break;
    }

    // Find free slot
    int slot = -1;
    for (int i = 0; i < H3_DYNAMIC_TABLE_MAX_ENTRIES; i++) {
        if (!dt->entries[i].active) { slot = i; break; }
    }
    if (slot < 0) return 0; // table full

    uint64_t abs_idx = dt->next_absolute_index;
    dt->next_absolute_index++;
    dt->total_inserted++;
    dt->largest_ref = dt->total_inserted - 1;

    // Extra eviction safety (shouldn't be needed after loop above)
    while (dt->current_size + entry_size > dt->max_capacity) {
        int found = 0;
        for (int i = 0; i < H3_DYNAMIC_TABLE_MAX_ENTRIES; i++) {
            if (dt->entries[i].active) {
                size_t ev_sz = strlen(dt->entries[i].name) + strlen(dt->entries[i].value) + 32;
                dt->current_size = dt->current_size > ev_sz ? dt->current_size - ev_sz : 0;
                dt->entries[i].active = 0;
                dt->entries[i].name[0] = '\0';
                dt->entries[i].value[0] = '\0';
                found = 1;
                break;
            }
        }
        if (!found) break;
    }

    dt->entries[slot].active = 1;
    dt->entries[slot].index = abs_idx;
    strncpy(dt->entries[slot].name, name, sizeof(dt->entries[slot].name) - 1);
    dt->entries[slot].name[sizeof(dt->entries[slot].name) - 1] = '\0';
    strncpy(dt->entries[slot].value, value, sizeof(dt->entries[slot].value) - 1);
    dt->entries[slot].value[sizeof(dt->entries[slot].value) - 1] = '\0';
    dt->current_size += entry_size;
    return 1;
}

/// Look up by absolute index. Returns pointer to entry, or NULL.
static const H3DynamicTableEntry *h3_dt_lookup_absolute(
    const H3DynamicTable *dt, uint64_t abs_idx) {
    for (int i = 0; i < H3_DYNAMIC_TABLE_MAX_ENTRIES; i++) {
        if (dt->entries[i].active && dt->entries[i].index == abs_idx) {
            return &dt->entries[i];
        }
    }
    return NULL;
}

/// Look up by post-base index (RFC 9204 Section 4.5.3).
/// Post-base index 0 = most recently inserted (largest_ref).
/// Post-base index N = entry with absolute_index = largest_ref - N.
/// NB7-61 parity: dual-bound check (total_inserted + active count).
static const H3DynamicTableEntry *h3_dt_lookup_post_base(
    const H3DynamicTable *dt, uint64_t post_base_idx) {
    if (h3_dt_largest_ref(dt) == 0 && post_base_idx == 0 && h3_dt_is_empty(dt)) {
        return NULL;
    }
    if (post_base_idx >= h3_dt_total_inserted(dt)) return NULL;
    uint64_t abs = h3_dt_largest_ref(dt) > post_base_idx
        ? h3_dt_largest_ref(dt) - post_base_idx : 0;
    if (abs >= h3_dt_total_inserted(dt)) return NULL;
    return h3_dt_lookup_absolute(dt, abs);
}

/// Duplicate existing entry by re-inserting it.
static int h3_dt_duplicate(H3DynamicTable *dt, uint64_t source_index) {
    const H3DynamicTableEntry *src = h3_dt_lookup_absolute(dt, source_index);
    if (!src) return 0;
    return h3_dt_insert(dt, src->name, src->value);
}

/// Set new capacity, evicting entries if needed.
static void h3_dt_set_capacity(H3DynamicTable *dt, size_t new_capacity) {
    if (new_capacity < dt->max_capacity) {
        h3_dt_evict_to_capacity(dt, new_capacity);
    } else {
        dt->max_capacity = new_capacity;
    }
}

/// Convert relative index to absolute.
/// Relative index 0 = most recently inserted entry.
static int h3_dt_relative_to_absolute(
    const H3DynamicTable *dt, uint64_t relative_idx, uint64_t *out_abs) {
    if (relative_idx >= h3_dt_total_inserted(dt) || h3_dt_is_empty(dt)) return 0;
    uint64_t abs = h3_dt_largest_ref(dt) > relative_idx
        ? h3_dt_largest_ref(dt) - relative_idx : 0;
    if (!h3_dt_lookup_absolute(dt, abs)) return 0;
    *out_abs = abs;
    return 1;
}

// ── QPACK integer coding (RFC 9204 Section 4.1.1) ────────────────────────
// QPACK uses the same integer coding as HPACK (RFC 7541 Section 5.1) but
// may use different prefix sizes.

static int h3_qpack_decode_int(const unsigned char *data, size_t data_len,
                                uint8_t prefix_bits, uint64_t *out, size_t *consumed) {
    if (data_len == 0) return -1;
    // Guard against prefix_bits == 8 overflow: (1u8 << 8) wraps to 0.
    uint8_t mask = (prefix_bits >= 8) ? 0xFF : (uint8_t)((1 << prefix_bits) - 1);
    uint64_t val = data[0] & mask;
    if (val < (uint64_t)mask) {
        *out = val;
        *consumed = 1;
        return 0;
    }
    // Multi-byte
    uint64_t m = 0;
    for (size_t i = 1; i < data_len; i++) {
        val += ((uint64_t)(data[i] & 0x7F)) << m;
        m += 7;
        if (!(data[i] & 0x80)) {
            *out = val;
            *consumed = i + 1;
            return 0;
        }
        if (m > 62) return -1; // overflow protection
    }
    return -1; // incomplete
}

static int h3_qpack_encode_int(unsigned char *buf, size_t buf_cap,
                                uint8_t prefix_bits, uint64_t value,
                                uint8_t prefix_byte, size_t *written) {
    if (buf_cap == 0) return -1;
    uint8_t mask = (uint8_t)((1 << prefix_bits) - 1);
    if (value < (uint64_t)mask) {
        buf[0] = prefix_byte | (uint8_t)value;
        *written = 1;
        return 0;
    }
    buf[0] = prefix_byte | mask;
    value -= mask;
    size_t pos = 1;
    while (value >= 128) {
        if (pos >= buf_cap) return -1;
        buf[pos++] = (uint8_t)((value & 0x7F) | 0x80);
        value >>= 7;
    }
    if (pos >= buf_cap) return -1;
    buf[pos++] = (uint8_t)value;
    *written = pos;
    return 0;
}

// ── QPACK string coding ──────────────────────────────────────────────────
// QPACK Section 4.1.2: string literals use the same format as HPACK.
// We reuse the H2 Huffman decode for QPACK since the Huffman table is identical.
// For simplicity in Phase 2, we encode strings as plain (non-Huffman) literals.

static int h3_qpack_decode_string(const unsigned char *data, size_t data_len,
                                   char *out, size_t out_cap, size_t *consumed) {
    if (data_len == 0) return -1;
    int is_huffman = (data[0] & 0x80) != 0;
    uint64_t str_len;
    size_t int_consumed;
    if (h3_qpack_decode_int(data, data_len, 7, &str_len, &int_consumed) < 0) return -1;
    if (int_consumed + (size_t)str_len > data_len) return -1;
    const unsigned char *str_data = data + int_consumed;

    if (is_huffman) {
        // Reuse H2 Huffman decode
        int dec_len = h2_huffman_decode(str_data, (size_t)str_len, out, out_cap - 1);
        if (dec_len < 0) return -1;
        out[dec_len] = '\0';
    } else {
        if ((size_t)str_len >= out_cap) return -1;
        memcpy(out, str_data, (size_t)str_len);
        out[(size_t)str_len] = '\0';
    }
    *consumed = int_consumed + (size_t)str_len;
    return 0;
}

static int h3_qpack_encode_string(unsigned char *buf, size_t buf_cap, const char *s) {
    // Phase 2: plain (non-Huffman) encoding for simplicity.
    size_t slen = strlen(s);
    size_t int_written;
    if (h3_qpack_encode_int(buf, buf_cap, 7, (uint64_t)slen, 0x00, &int_written) < 0) return -1;
    if (int_written + slen > buf_cap) return -1;
    memcpy(buf + int_written, s, slen);
    return (int)(int_written + slen);
}

// ── QPACK header block decode (RFC 9204 Section 4.5) ──────────────────────
// v7 QPACK scope: static + dynamic table (NET7-10d).
// The decode_block now accepts an optional dynamic table parameter.

// Reuse H2Header for H3 headers (same name/value buffer structure).
typedef H2Header H3Header;

// NB7-104: truncation-safe string copy for H3Header fields.
// snprintf silently truncates; this macro detects it and returns -1.
#define H3_STRCPY(dst, src) do { \
    int _n = snprintf((dst), sizeof(dst), "%s", (src)); \
    if (_n < 0 || (size_t)_n >= sizeof(dst)) return -1; \
} while (0)

/// Decode a QPACK header block with optional dynamic table (NET7-10d).
/// If dynamic_table is NULL, behaves as static-table-only (legacy mode).
static int h3_qpack_decode_block_with_dt(const unsigned char *data, size_t data_len,
                                  H3Header *headers, int max_headers,
                                  const H3DynamicTable *dynamic_table) {
    if (data_len < 2) return -1;

    // Required Insert Count (prefix int, 8-bit prefix)
    uint64_t req_insert_count;
    size_t consumed;
    if (h3_qpack_decode_int(data, data_len, 8, &req_insert_count, &consumed) < 0) return -1;

    // If dynamic table is required but not provided, reject.
    if (req_insert_count != 0 && dynamic_table == NULL) return -1;

    // Sign bit + Delta Base (prefix int, 7-bit prefix) — RFC 9204 Section 4.5.1.
    // NB7-109 fix: extract sign bit and compute D_abs per RFC 9204 §4.5.1.
    // MostDeltasBase: bit 6 is Sign, bits 5-0 are D (6-bit value).
    if (consumed >= data_len) return -1;
    uint64_t most_deltas_base;
    size_t db_consumed;
    if (h3_qpack_decode_int(data + consumed, data_len - consumed, 7, &most_deltas_base, &db_consumed) < 0) return -1;
    consumed += db_consumed;
    int sign_bit = (most_deltas_base >> 6) != 0;
    uint64_t delta_base = most_deltas_base & 0x3F;
    /* D_abs = D when Sign=0, D + 2^(N-1) when Sign=1 */
    uint64_t d_abs = delta_base;
    if (sign_bit && req_insert_count > 0 && req_insert_count <= 63) {
        uint64_t pow2 = (uint64_t)1 << (req_insert_count - 1);
        d_abs = delta_base + pow2;
    }

    int hdr_count = 0;
    while (consumed < data_len) {
        if (hdr_count >= max_headers) return -1;
        uint8_t byte = data[consumed];

        if (byte & 0x80) {
            // Indexed Field Line (Section 4.5.2): 1Txxxxxx
            int is_static = (byte & 0x40) != 0;
            uint64_t index;
            size_t idx_consumed;
            if (h3_qpack_decode_int(data + consumed, data_len - consumed, 6, &index, &idx_consumed) < 0) return -1;
            consumed += idx_consumed;

            if (is_static) {
                if (index >= H3_QPACK_STATIC_TABLE_LEN) return -1;
                H3_STRCPY(headers[hdr_count].name, H3_QPACK_STATIC_TABLE[index].name);
                H3_STRCPY(headers[hdr_count].value, H3_QPACK_STATIC_TABLE[index].value);
            } else {
                /* NB7-109 fix: Dynamic table indexed (Before Base, T=0)
                 * absolute_index = RIC - D_abs - 1 - index
                 * per RFC 9204 §4.5.1 + §4.5.2 */
                if (!dynamic_table || h3_dt_is_empty(dynamic_table)) return -1;
                if (req_insert_count == 0) return -1;
                if (index >= d_abs + 1) return -1;
                if (req_insert_count < d_abs + 1) return -1;
                uint64_t base_val = req_insert_count - d_abs - 1;
                if (base_val < index) return -1;
                uint64_t abs = base_val - index;
                const H3DynamicTableEntry *entry = h3_dt_lookup_absolute(dynamic_table, abs);
                if (!entry) return -1;
                H3_STRCPY(headers[hdr_count].name, entry->name);
                H3_STRCPY(headers[hdr_count].value, entry->value);
            }
            hdr_count++;
        } else if (byte & 0x40) {
            // Literal Field Line With Name Reference (Section 4.5.4): 01NTxxxx
            int is_never_indexed = (byte & 0x20) != 0;
            (void)is_never_indexed;
            int is_static = (byte & 0x10) != 0;
            uint64_t name_index;
            size_t ni_consumed;
            if (h3_qpack_decode_int(data + consumed, data_len - consumed, 4, &name_index, &ni_consumed) < 0) return -1;
            consumed += ni_consumed;

            if (is_static) {
                if (name_index >= H3_QPACK_STATIC_TABLE_LEN) return -1;
                H3_STRCPY(headers[hdr_count].name, H3_QPACK_STATIC_TABLE[name_index].name);
            } else {
                /* NB7-109 fix: Dynamic table name reference (Before Base, T=0)
                 * absolute_index = RIC - D_abs - 1 - name_index
                 * per RFC 9204 §4.5.1 + §4.5.4 */
                if (!dynamic_table || h3_dt_is_empty(dynamic_table)) return -1;
                if (name_index >= d_abs + 1) return -1;
                if (req_insert_count < d_abs + 1) return -1;
                uint64_t base_val = req_insert_count - d_abs - 1;
                if (base_val < name_index) return -1;
                uint64_t abs = base_val - name_index;
                const H3DynamicTableEntry *entry = h3_dt_lookup_absolute(dynamic_table, abs);
                if (!entry) return -1;
                H3_STRCPY(headers[hdr_count].name, entry->name);
            }

            // Value string
            size_t val_consumed;
            if (h3_qpack_decode_string(data + consumed, data_len - consumed,
                                        headers[hdr_count].value, sizeof(headers[hdr_count].value),
                                        &val_consumed) < 0) return -1;
            consumed += val_consumed;
            hdr_count++;
        } else if (byte & 0x20) {
            // Literal Field Line With Literal Name (Section 4.5.6): 001Nxxxx
            int name_huffman = (byte & 0x08) != 0;
            uint64_t name_len;
            size_t nli_consumed;
            if (h3_qpack_decode_int(data + consumed, data_len - consumed, 3, &name_len, &nli_consumed) < 0) return -1;
            consumed += nli_consumed;
            if (consumed + (size_t)name_len > data_len) return -1;
            if (name_huffman) {
                int dec = h2_huffman_decode(data + consumed, (size_t)name_len,
                                           headers[hdr_count].name, sizeof(headers[hdr_count].name) - 1);
                if (dec < 0) return -1;
                headers[hdr_count].name[dec] = '\0';
            } else {
                if ((size_t)name_len >= sizeof(headers[hdr_count].name)) return -1;
                memcpy(headers[hdr_count].name, data + consumed, (size_t)name_len);
                headers[hdr_count].name[(size_t)name_len] = '\0';
            }
            consumed += (size_t)name_len;

            // Decode value: standard QPACK string (7-bit prefix)
            size_t val_consumed;
            if (h3_qpack_decode_string(data + consumed, data_len - consumed,
                                        headers[hdr_count].value, sizeof(headers[hdr_count].value),
                                        &val_consumed) < 0) return -1;
            consumed += val_consumed;
            hdr_count++;
        } else if (byte & 0x10) {
            // Indexed Field Line With Post-Base Index (Section 4.5.3): 0001xxxx
            // NET7-10d: dynamic table post-base reference
            // At this point bits 7,6,5 are all 0. Bit 4 (0x10) distinguishes
            // post-base indexed (1) from post-base literal name reference (0).
            if (!dynamic_table || h3_dt_is_empty(dynamic_table)) return -1;
            uint64_t post_base_index;
            size_t idx_consumed;
            if (h3_qpack_decode_int(data + consumed, data_len - consumed, 4, &post_base_index, &idx_consumed) < 0) return -1;
            consumed += idx_consumed;

            const H3DynamicTableEntry *entry = h3_dt_lookup_post_base(dynamic_table, post_base_index);
            if (!entry) return -1;
            H3_STRCPY(headers[hdr_count].name, entry->name);
            H3_STRCPY(headers[hdr_count].value, entry->value);
            hdr_count++;
        } else {
            // NB7-110: Literal Field Line With Post-Base Name Reference (Section 4.5.5)
            // Wire format: 000N xxxx where N = Never-Indexed bit (bit 3).
            // Bits 3-0 + continuation form a prefix integer for the name index.
            //   N=1: name from static table index
            //   N=0: name from dynamic table post-base index
            int never_indexed = (byte & 0x08) != 0;
            uint64_t name_index;
            size_t ni_consumed;
            if (h3_qpack_decode_int(data + consumed, data_len - consumed, 3, &name_index, &ni_consumed) < 0) return -1;
            consumed += ni_consumed;

            if (never_indexed) {
                // Static table name reference
                if (name_index >= H3_QPACK_STATIC_TABLE_LEN) return -1;
                H3_STRCPY(headers[hdr_count].name, H3_QPACK_STATIC_TABLE[name_index].name);
            } else {
                // Dynamic table post-base name reference
                if (!dynamic_table || h3_dt_is_empty(dynamic_table)) return -1;
                if (req_insert_count == 0) return -1;
                const H3DynamicTableEntry *entry = h3_dt_lookup_post_base(dynamic_table, name_index);
                if (!entry) return -1;
                H3_STRCPY(headers[hdr_count].name, entry->name);
            }

            // Value string
            size_t val_consumed;
            if (h3_qpack_decode_string(data + consumed, data_len - consumed,
                                        headers[hdr_count].value, sizeof(headers[hdr_count].value),
                                        &val_consumed) < 0) return -1;
            consumed += val_consumed;
            hdr_count++;
        }
    }
    return hdr_count;
}

/// Original decode_block signature (static-table-only for backward compat).
static int h3_qpack_decode_block(const unsigned char *data, size_t data_len,
                                  H3Header *headers, int max_headers) {
    return h3_qpack_decode_block_with_dt(data, data_len, headers, max_headers, NULL);
}

// ── QPACK header block encode (RFC 9204 Section 4.5) ──────────────────────
// Phase 2: encode using static table references where possible, literal otherwise.
// Always uses Required Insert Count = 0 (no dynamic table).

static int h3_qpack_encode_block(unsigned char *buf, size_t buf_cap,
                                  int status, const H3Header *headers, int count) {
    size_t pos = 0;
    // Required Insert Count = 0 (1 byte: 0x00)
    if (pos >= buf_cap) return -1;
    buf[pos++] = 0x00;
    // Delta Base = 0 with sign=0 (1 byte: 0x00)
    if (pos >= buf_cap) return -1;
    buf[pos++] = 0x00;

    // Encode :status pseudo-header
    // Try static table index for common status codes
    int status_idx = -1;
    switch (status) {
        case 100: status_idx = 63; break;
        case 103: status_idx = 24; break;
        case 200: status_idx = 25; break;
        case 204: status_idx = 64; break;
        case 206: status_idx = 65; break;
        case 302: status_idx = 66; break;
        case 304: status_idx = 26; break;
        case 400: status_idx = 67; break;
        case 403: status_idx = 68; break;
        case 404: status_idx = 27; break;
        case 421: status_idx = 69; break;
        case 425: status_idx = 70; break;
        case 500: status_idx = 71; break;
        case 503: status_idx = 28; break;
        default: break;
    }

    if (status_idx >= 0) {
        // Indexed Field Line: 11xxxxxx (T=1 for static)
        size_t iw;
        if (h3_qpack_encode_int(buf + pos, buf_cap - pos, 6, (uint64_t)status_idx, 0xC0, &iw) < 0) return -1;
        pos += iw;
    } else {
        // Literal with name reference to :status (static index varies by status)
        // Use QPACK static table index 25 for ":status" name reference (any :status entry works)
        // Instruction: 0101xxxx (N=0, T=1 for static, 4-bit prefix)
        size_t niw;
        if (h3_qpack_encode_int(buf + pos, buf_cap - pos, 4, 25, 0x50, &niw) < 0) return -1;
        pos += niw;
        // Value: status code as string
        char status_str[16];
        snprintf(status_str, sizeof(status_str), "%d", status);
        int sw = h3_qpack_encode_string(buf + pos, buf_cap - pos, status_str);
        if (sw < 0) return -1;
        pos += (size_t)sw;
    }

    // Encode regular headers
    for (int i = 0; i < count; i++) {
        // Try to find name-only match in static table
        int name_idx = -1;
        for (size_t j = 0; j < H3_QPACK_STATIC_TABLE_LEN; j++) {
            if (strcasecmp(H3_QPACK_STATIC_TABLE[j].name, headers[i].name) == 0) {
                // Check for full match (name + value)
                if (strcmp(H3_QPACK_STATIC_TABLE[j].value, headers[i].value) == 0) {
                    // Full match: indexed field line
                    size_t iw;
                    if (h3_qpack_encode_int(buf + pos, buf_cap - pos, 6, (uint64_t)j, 0xC0, &iw) < 0) return -1;
                    pos += iw;
                    name_idx = -2; // sentinel: fully encoded
                    break;
                }
                if (name_idx < 0) name_idx = (int)j; // first name match
            }
        }
        if (name_idx == -2) continue; // already encoded

        if (name_idx >= 0) {
            // Literal with static name reference: 0101xxxx
            size_t niw;
            if (h3_qpack_encode_int(buf + pos, buf_cap - pos, 4, (uint64_t)name_idx, 0x50, &niw) < 0) return -1;
            pos += niw;
            int vw = h3_qpack_encode_string(buf + pos, buf_cap - pos, headers[i].value);
            if (vw < 0) return -1;
            pos += (size_t)vw;
        } else {
            // Literal with literal name: 0010xxxx
            if (pos >= buf_cap) return -1;
            buf[pos] = 0x20; // instruction byte (N=0, H=0 for name)
            // Encode name (3-bit prefix for length, but instruction byte already placed)
            size_t name_len = strlen(headers[i].name);
            size_t nliw;
            if (h3_qpack_encode_int(buf + pos, buf_cap - pos, 3, (uint64_t)name_len, 0x20, &nliw) < 0) return -1;
            pos += nliw;
            if (pos + name_len > buf_cap) return -1;
            memcpy(buf + pos, headers[i].name, name_len);
            pos += name_len;
            // Encode value
            int vw = h3_qpack_encode_string(buf + pos, buf_cap - pos, headers[i].value);
            if (vw < 0) return -1;
            pos += (size_t)vw;
        }
    }

    return (int)pos;
}

// ── H3 stream state ───────────────────────────────────────────────────────

#define H3_STREAM_IDLE              0
#define H3_STREAM_OPEN              1
#define H3_STREAM_HALF_CLOSED_LOCAL 2
#define H3_STREAM_CLOSED            3

typedef struct {
    uint64_t stream_id;
    int state;
    H3Header *request_headers;
    int request_header_count;
    unsigned char *request_body;
    size_t request_body_len;
    size_t request_body_cap;
} H3Stream;

typedef struct {
    H3Stream streams[H3_MAX_STREAMS];
    int stream_count;
    uint64_t max_field_section_size;
    uint64_t last_peer_stream_id;
    int goaway_sent;
    uint64_t goaway_id; // stream ID sent in GOAWAY
} H3Conn;

static H3Stream *h3_conn_find_stream(H3Conn *conn, uint64_t stream_id) {
    for (int i = conn->stream_count - 1; i >= 0; i--) {
        if (conn->streams[i].stream_id == stream_id) return &conn->streams[i];
    }
    return NULL;
}

static H3Stream *h3_conn_new_stream(H3Conn *conn, uint64_t stream_id) {
    if (conn->stream_count >= H3_MAX_STREAMS) return NULL;
    H3Stream *s = &conn->streams[conn->stream_count++];
    memset(s, 0, sizeof(*s));
    s->stream_id = stream_id;
    s->state = H3_STREAM_OPEN;
    return s;
}

static void h3_stream_free(H3Stream *s) {
    free(s->request_headers);
    s->request_headers = NULL;
    free(s->request_body);
    s->request_body = NULL;
}

static void h3_conn_remove_closed_streams(H3Conn *conn) {
    int new_count = 0;
    for (int i = 0; i < conn->stream_count; i++) {
        if (conn->streams[i].state != H3_STREAM_CLOSED) {
            if (i != new_count) conn->streams[new_count] = conn->streams[i];
            new_count++;
        } else {
            h3_stream_free(&conn->streams[i]);
        }
    }
    conn->stream_count = new_count;
}

static void h3_conn_init(H3Conn *conn) {
    memset(conn, 0, sizeof(*conn));
    conn->max_field_section_size = H3_DEFAULT_MAX_FIELD_SECTION_SIZE;
    conn->goaway_sent = 0;
}

static void h3_conn_free(H3Conn *conn) {
    for (int i = 0; i < conn->stream_count; i++) h3_stream_free(&conn->streams[i]);
    conn->stream_count = 0;
}

// ── H3 variable-length integer coding (RFC 9000 Section 16) ───────────────
// QUIC uses a different variable-length integer format than HPACK/QPACK.
// 2-bit prefix: 00=1byte, 01=2byte, 10=4byte, 11=8byte.

static int h3_varint_decode(const unsigned char *data, size_t data_len,
                             uint64_t *out, size_t *consumed) {
    if (data_len == 0) return -1;
    uint8_t prefix = data[0] >> 6;
    size_t len = (size_t)1 << prefix;
    if (data_len < len) return -1;
    uint64_t val = data[0] & 0x3F;
    for (size_t i = 1; i < len; i++) {
        val = (val << 8) | data[i];
    }

    // NET7-5a: Reject non-canonical encoding (RFC 9000 Section 16).
    // Values that could fit in fewer bytes but use a larger encoding are malformed.
    switch (prefix) {
        case 1: if (val <= 63)      return -1; break;  // 2-byte encoding
        case 2: if (val <= 16383)   return -1; break;  // 4-byte encoding
        case 3: if (val <= 1073741823ULL) return -1; break;  // 8-byte encoding
        default: /* 1-byte always valid */              break;
    }

    *out = val;
    *consumed = len;
    return 0;
}

static int h3_varint_encode(unsigned char *buf, size_t buf_cap,
                             uint64_t value, size_t *written) {
    if (value <= 63) {
        if (buf_cap < 1) return -1;
        buf[0] = (uint8_t)value;
        *written = 1;
    } else if (value <= 16383) {
        if (buf_cap < 2) return -1;
        buf[0] = (uint8_t)(0x40 | (value >> 8));
        buf[1] = (uint8_t)(value & 0xFF);
        *written = 2;
    } else if (value <= 1073741823ULL) {
        if (buf_cap < 4) return -1;
        buf[0] = (uint8_t)(0x80 | (value >> 24));
        buf[1] = (uint8_t)((value >> 16) & 0xFF);
        buf[2] = (uint8_t)((value >> 8) & 0xFF);
        buf[3] = (uint8_t)(value & 0xFF);
        *written = 4;
    } else {
        if (buf_cap < 8) return -1;
        buf[0] = (uint8_t)(0xC0 | (value >> 56));
        buf[1] = (uint8_t)((value >> 48) & 0xFF);
        buf[2] = (uint8_t)((value >> 40) & 0xFF);
        buf[3] = (uint8_t)((value >> 32) & 0xFF);
        buf[4] = (uint8_t)((value >> 24) & 0xFF);
        buf[5] = (uint8_t)((value >> 16) & 0xFF);
        buf[6] = (uint8_t)((value >> 8) & 0xFF);
        buf[7] = (uint8_t)(value & 0xFF);
        *written = 8;
    }
    return 0;
}

// ── H3 frame I/O ──────────────────────────────────────────────────────────
// H3 frames use QUIC variable-length integers for type and length.
// Frame format: Type (varint) + Length (varint) + Payload

// Encode an H3 frame into a buffer.
// Returns total frame size written, or -1 on error.
static int h3_encode_frame(unsigned char *buf, size_t buf_cap,
                            uint64_t frame_type, const unsigned char *payload, size_t payload_len) {
    size_t pos = 0;
    size_t tw, lw;
    if (h3_varint_encode(buf + pos, buf_cap - pos, frame_type, &tw) < 0) return -1;
    pos += tw;
    if (h3_varint_encode(buf + pos, buf_cap - pos, (uint64_t)payload_len, &lw) < 0) return -1;
    pos += lw;
    if (pos + payload_len > buf_cap) return -1;
    if (payload_len > 0) memcpy(buf + pos, payload, payload_len);
    return (int)(pos + payload_len);
}

// Decode an H3 frame header (type + length) from a buffer.
// Returns 0 on success, -1 on error.
// NET7-5a hardening: validates that declared frame_length fits within available data.
static int h3_decode_frame_header(const unsigned char *data, size_t data_len,
                                   uint64_t *frame_type, uint64_t *frame_length,
                                   size_t *header_size) {
    size_t tc, lc;
    if (h3_varint_decode(data, data_len, frame_type, &tc) < 0) return -1;
    if (h3_varint_decode(data + tc, data_len - tc, frame_length, &lc) < 0) return -1;
    *header_size = tc + lc;
    // NB7-24 portability guard: reject frame_length that exceeds SIZE_MAX.
    // 64-bit onlyの場合は常に安全。32-bit systemでもusize overflowをgraceful reject。
    if (*frame_length > (uint64_t)(SIZE_MAX)) return -1;
    // NET7-5a: Bounded-copy — declared payload length must not exceed available data.
    // Rejects malformed frames where frame_length > remaining buffer (truncation or attack).
    if (*header_size + (size_t)*frame_length > data_len) return -1;
    return 0;
}

// Maximum SETTINGS pairs before rejection (NET7-5a hardening).
// RFC 9114 does not specify a maximum. 64 is a reasonable DoS mitigation limit
// (typical servers send 3-5 pairs). NB7-31, NB7-37
#define H3_MAX_SETTINGS_PAIRS 64

// ── H3 SETTINGS encode/decode ─────────────────────────────────────────────

// Encode a SETTINGS frame payload (varint pairs).
// Phase 2: send QPACK_MAX_TABLE_CAPACITY=0, QPACK_BLOCKED_STREAMS=0
// (static-only QPACK, no dynamic table).
static int h3_encode_settings(unsigned char *buf, size_t buf_cap) {
    size_t pos = 0;
    size_t w;
    // QPACK_MAX_TABLE_CAPACITY = 0
    if (h3_varint_encode(buf + pos, buf_cap - pos, H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY, &w) < 0) return -1;
    pos += w;
    if (h3_varint_encode(buf + pos, buf_cap - pos, 0, &w) < 0) return -1;
    pos += w;
    // QPACK_BLOCKED_STREAMS = 0
    if (h3_varint_encode(buf + pos, buf_cap - pos, H3_SETTINGS_QPACK_BLOCKED_STREAMS, &w) < 0) return -1;
    pos += w;
    if (h3_varint_encode(buf + pos, buf_cap - pos, 0, &w) < 0) return -1;
    pos += w;
    return (int)pos;
}

// Decode SETTINGS frame payload.
// NET7-5a hardening: bounded iteration to prevent DoS via oversized SETTINGS frame.
static int h3_decode_settings(H3Conn *conn, const unsigned char *data, size_t data_len) {
    size_t pos = 0;
    int pair_count = 0;
    while (pos < data_len) {
        // NET7-5a: bounded iteration
        if (pair_count >= H3_MAX_SETTINGS_PAIRS) return -1;
        pair_count += 1;
        uint64_t id, val;
        size_t ic, vc;
        if (h3_varint_decode(data + pos, data_len - pos, &id, &ic) < 0) return -1;
        pos += ic;
        if (h3_varint_decode(data + pos, data_len - pos, &val, &vc) < 0) return -1;
        pos += vc;
        switch (id) {
            case H3_SETTINGS_QPACK_MAX_TABLE_CAPACITY:
                // Phase 2: we only support static table, ignore capacity > 0
                break;
            case H3_SETTINGS_MAX_FIELD_SECTION_SIZE:
                conn->max_field_section_size = val;
                break;
            case H3_SETTINGS_QPACK_BLOCKED_STREAMS:
                // Phase 2: no blocked streams support
                break;
            default:
                // Unknown settings are ignored (RFC 9114 Section 7.2.4)
                break;
        }
    }
    return 0;
}

// ── H3 GOAWAY encode ──────────────────────────────────────────────────────

static int h3_encode_goaway(unsigned char *buf, size_t buf_cap, uint64_t stream_id) {
    // GOAWAY payload is a single varint (stream ID)
    unsigned char payload[8];
    size_t pw;
    if (h3_varint_encode(payload, sizeof(payload), stream_id, &pw) < 0) return -1;
    return h3_encode_frame(buf, buf_cap, H3_FRAME_GOAWAY, payload, pw);
}

// ── H3 request extraction ─────────────────────────────────────────────────
// Mirrors h2_extract_request_fields but for H3 pseudo-headers.

typedef struct {
    char method[16];
    char path[2048];
    char authority[256];
    H3Header *regular_headers;
    int regular_count;
    int ok;
    int error_reason;
} H3RequestFields;

#define H3_REQ_ERR_NONE             0
#define H3_REQ_ERR_ORDERING         1
#define H3_REQ_ERR_UNKNOWN_PSEUDO   2
#define H3_REQ_ERR_MISSING_PSEUDO   3
#define H3_REQ_ERR_DUPLICATE_PSEUDO 4
#define H3_REQ_ERR_EMPTY_PSEUDO     5
// C27B-026 Step 3 Option B: pseudo-header value exceeds the wire-byte
// upper limit. Mirrors H2_REQ_ERR_PSEUDO_TOO_LONG in net_h1_h2.c and
// the parser-side rejects in src/interpreter/net_eval/h1.rs.
#define H3_REQ_ERR_PSEUDO_TOO_LONG  6

// C27B-026 Step 3 Option B helper: bounded copy via memcpy + pre-
// length check (gcc cannot follow snprintf-with-runtime-check, so
// the -Wformat-truncation warning only stays silent for the memcpy
// form). Mirrors H2_COPY_PSEUDO in net_h1_h2.c. Used inside
// h3_extract_request_fields below.
#define H3_COPY_PSEUDO(dst, dst_size, seen) do { \
    size_t v_len = strlen(headers[i].value); \
    if (v_len >= (dst_size)) { out->error_reason = H3_REQ_ERR_PSEUDO_TOO_LONG; free(regs); return; } \
    memcpy((dst), headers[i].value, v_len); (dst)[v_len] = '\0'; (seen) = 1; \
} while (0)

static void h3_extract_request_fields(const H3Header *headers, int count, H3RequestFields *out) {
    memset(out, 0, sizeof(*out));
    out->ok = 0;
    out->error_reason = H3_REQ_ERR_NONE;

    char scheme[16] = "";
    int saw_regular = 0;
    int saw_method = 0, saw_path = 0, saw_authority = 0, saw_scheme = 0;
    H3Header *regs = (H3Header*)TAIDA_MALLOC(sizeof(H3Header) * (size_t)(count + 1), "h3_regular_headers");
    if (!regs) return;
    int reg_count = 0;

    for (int i = 0; i < count; i++) {
        if (headers[i].name[0] == ':') {
            if (saw_regular) {
                out->error_reason = H3_REQ_ERR_ORDERING;
                free(regs);
                return;
            }
            // C27B-026 Step 3 Option B: bounded copy + cap check; see
            // H3_COPY_PSEUDO macro defined above for details.
            if (strcmp(headers[i].name, ":method") == 0) {
                if (saw_method) { out->error_reason = H3_REQ_ERR_DUPLICATE_PSEUDO; free(regs); return; }
                H3_COPY_PSEUDO(out->method, sizeof(out->method), saw_method);
            } else if (strcmp(headers[i].name, ":path") == 0) {
                if (saw_path) { out->error_reason = H3_REQ_ERR_DUPLICATE_PSEUDO; free(regs); return; }
                H3_COPY_PSEUDO(out->path, sizeof(out->path), saw_path);
            } else if (strcmp(headers[i].name, ":authority") == 0) {
                if (saw_authority) { out->error_reason = H3_REQ_ERR_DUPLICATE_PSEUDO; free(regs); return; }
                H3_COPY_PSEUDO(out->authority, sizeof(out->authority), saw_authority);
            } else if (strcmp(headers[i].name, ":scheme") == 0) {
                if (saw_scheme) { out->error_reason = H3_REQ_ERR_DUPLICATE_PSEUDO; free(regs); return; }
                H3_COPY_PSEUDO(scheme, sizeof(scheme), saw_scheme);
            } else {
                out->error_reason = H3_REQ_ERR_UNKNOWN_PSEUDO;
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

    // Required pseudo-headers: :method, :path, :scheme (matches H2 semantics)
    if (!saw_method || !saw_path || !saw_scheme) {
        out->error_reason = H3_REQ_ERR_MISSING_PSEUDO;
        free(regs);
        return;
    }

    // Reject empty pseudo-header values (matches H2 semantics)
    if (out->method[0] == '\0' || out->path[0] == '\0' || scheme[0] == '\0') {
        out->error_reason = H3_REQ_ERR_EMPTY_PSEUDO;
        free(regs);
        return;
    }

    out->regular_headers = regs;
    out->regular_count = reg_count;
    out->ok = 1;
}

// ── H3 request pack builder ───────────────────────────────────────────────
// Mirrors h2_build_request_pack but with version @(major: 3, minor: 0)
// and protocol "h3".

typedef struct {
    taida_val handler;
    int handler_arity;
    int64_t *request_count;
    int64_t max_requests;
    char peer_host[64];
    int peer_port;
} H3ServeCtx;

static taida_val h3_dispatch_request(H3ServeCtx *ctx, taida_val request_pack) {
    return taida_invoke_callback1(ctx->handler, request_pack);
}

static taida_val h3_build_request_pack(H3RequestFields *fields,
                                        const unsigned char *body, size_t body_len,
                                        const char *peer_host, int peer_port) {
    // D29B-011 (Track-ζ Lock-H, 2026-04-27): mirror h2_build_request_pack.
    // QPACK has the same dynamic-table reallocation problem as HPACK, so
    // we copy decoded pseudo / regular header bytes into a per-request
    // arena alongside the body, then expose every Str-shaped field as
    // span packs into that arena. This brings h3 to byte-identical span
    // shape with h1 / h2 and lets SpanEquals[req.method, req.raw, "GET"]()
    // succeed under h3 instead of silently returning false.
    //
    // Strategy V1-A (sub-Lock Phase-5_..._track-zeta_sub-Lock.md):
    // single arena layout
    //   [body | method | path | query | n1 v1 n2 v2 ... | "host" authority]

    // Split path and query first so we know their lengths for arena sizing.
    char path_part[2048], query_part[2048];
    const char *qmark = strchr(fields->path, '?');
    size_t path_part_len, query_part_len;
    if (qmark) {
        size_t plen = (size_t)(qmark - fields->path);
        if (plen >= sizeof(path_part)) plen = sizeof(path_part) - 1;
        memcpy(path_part, fields->path, plen);
        path_part[plen] = '\0';
        path_part_len = plen;
        snprintf(query_part, sizeof(query_part), "%s", qmark + 1);
        query_part_len = strlen(query_part);
    } else {
        snprintf(path_part, sizeof(path_part), "%s", fields->path);
        path_part_len = strlen(path_part);
        query_part[0] = '\0';
        query_part_len = 0;
    }

    size_t method_len = strlen(fields->method);

    // Compute arena capacity: body + method + path + query + every header
    // name/value, plus the synthesized host header when :authority is set.
    // The H3RequestFields layout caps regular_count at H3_MAX_HEADERS;
    // we use the same bound here.
    size_t arena_size = body_len + method_len + path_part_len + query_part_len;
    size_t header_lens[H3_MAX_HEADERS][2];
    for (int i = 0; i < fields->regular_count && i < H3_MAX_HEADERS; i++) {
        header_lens[i][0] = strlen(fields->regular_headers[i].name);
        header_lens[i][1] = strlen(fields->regular_headers[i].value);
        arena_size += header_lens[i][0] + header_lens[i][1];
    }
    size_t authority_len = strlen(fields->authority);
    int has_host = authority_len > 0 ? 1 : 0;
    if (has_host) {
        arena_size += 4 /* "host" */ + authority_len;
    }

    unsigned char *arena = (unsigned char*)TAIDA_MALLOC(arena_size > 0 ? arena_size : 1, "h3_arena");
    if (!arena) {
        // OOM: degrade to legacy form (body-only raw, Str packs).
        arena_size = 0;
    }

    // 1. body
    if (arena && body_len > 0) memcpy(arena, body, body_len);
    size_t cursor = body_len;

    // 2. method / path / query
    size_t method_start = cursor;
    if (arena && method_len > 0) memcpy(arena + cursor, fields->method, method_len);
    cursor += method_len;

    size_t path_start = cursor;
    if (arena && path_part_len > 0) memcpy(arena + cursor, path_part, path_part_len);
    cursor += path_part_len;

    size_t query_start = cursor;
    if (arena && query_part_len > 0) memcpy(arena + cursor, query_part, query_part_len);
    cursor += query_part_len;

    // 3. headers (regular)
    size_t header_starts[H3_MAX_HEADERS][2];
    for (int i = 0; i < fields->regular_count && i < H3_MAX_HEADERS; i++) {
        header_starts[i][0] = cursor;
        if (arena && header_lens[i][0] > 0) {
            memcpy(arena + cursor, fields->regular_headers[i].name, header_lens[i][0]);
        }
        cursor += header_lens[i][0];
        header_starts[i][1] = cursor;
        if (arena && header_lens[i][1] > 0) {
            memcpy(arena + cursor, fields->regular_headers[i].value, header_lens[i][1]);
        }
        cursor += header_lens[i][1];
    }

    // 4. :authority -> host header
    size_t host_name_start = 0, host_value_start = 0;
    if (has_host) {
        host_name_start = cursor;
        if (arena) memcpy(arena + cursor, "host", 4);
        cursor += 4;
        host_value_start = cursor;
        if (arena && authority_len > 0) memcpy(arena + cursor, fields->authority, authority_len);
        cursor += authority_len;
    }

    // 5. raw = Bytes(arena). Free the staging arena after taida_bytes_from_raw
    // memcpys it into the taida_val[] slot layout.
    taida_val raw_bytes;
    if (arena) {
        raw_bytes = taida_bytes_from_raw(arena, (taida_val)arena_size);
        free(arena);
    } else {
        raw_bytes = taida_bytes_from_raw(body, (taida_val)body_len);
    }

    // 6. Header list: span packs into the arena (or fall back to Str on OOM)
    taida_val hdr_list = taida_list_new();
    for (int i = 0; i < fields->regular_count && i < H3_MAX_HEADERS; i++) {
        taida_val entry = taida_pack_new(2);
        taida_pack_set_hash(entry, 0, taida_str_hash((taida_val)"name"));
        taida_pack_set_hash(entry, 1, taida_str_hash((taida_val)"value"));
        if (arena_size > 0) {
            taida_pack_set(entry, 0, taida_net_make_span(
                (taida_val)header_starts[i][0], (taida_val)header_lens[i][0]));
            taida_pack_set_tag(entry, 0, TAIDA_TAG_PACK);
            taida_pack_set(entry, 1, taida_net_make_span(
                (taida_val)header_starts[i][1], (taida_val)header_lens[i][1]));
            taida_pack_set_tag(entry, 1, TAIDA_TAG_PACK);
        } else {
            taida_pack_set(entry, 0, (taida_val)taida_str_new_copy(
                fields->regular_headers[i].name));
            taida_pack_set_tag(entry, 0, TAIDA_TAG_STR);
            taida_pack_set(entry, 1, (taida_val)taida_str_new_copy(
                fields->regular_headers[i].value));
            taida_pack_set_tag(entry, 1, TAIDA_TAG_STR);
        }
        hdr_list = taida_list_append(hdr_list, entry);
    }
    if (has_host) {
        taida_val entry = taida_pack_new(2);
        taida_pack_set_hash(entry, 0, taida_str_hash((taida_val)"name"));
        taida_pack_set_hash(entry, 1, taida_str_hash((taida_val)"value"));
        if (arena_size > 0) {
            taida_pack_set(entry, 0, taida_net_make_span((taida_val)host_name_start, 4));
            taida_pack_set_tag(entry, 0, TAIDA_TAG_PACK);
            taida_pack_set(entry, 1, taida_net_make_span(
                (taida_val)host_value_start, (taida_val)authority_len));
            taida_pack_set_tag(entry, 1, TAIDA_TAG_PACK);
        } else {
            taida_pack_set(entry, 0, (taida_val)taida_str_new_copy("host"));
            taida_pack_set_tag(entry, 0, TAIDA_TAG_STR);
            taida_pack_set(entry, 1, (taida_val)taida_str_new_copy(fields->authority));
            taida_pack_set_tag(entry, 1, TAIDA_TAG_STR);
        }
        hdr_list = taida_list_append(hdr_list, entry);
    }

    // version pack @(major: 3, minor: 0) — HTTP/3
    taida_val version_pack = taida_pack_new(2);
    taida_pack_set_hash(version_pack, 0, taida_str_hash((taida_val)"major"));
    taida_pack_set(version_pack, 0, (taida_val)3);
    taida_pack_set_tag(version_pack, 0, TAIDA_TAG_INT);
    taida_pack_set_hash(version_pack, 1, taida_str_hash((taida_val)"minor"));
    taida_pack_set(version_pack, 1, (taida_val)0);
    taida_pack_set_tag(version_pack, 1, TAIDA_TAG_INT);

    // 14-field request pack (matches h2 structure for handler contract compatibility)
    taida_val req = taida_pack_new(14);
    int f = 0;
    #define SET_FIELD_H3(nm, val, tag) do { \
        taida_pack_set_hash(req, f, taida_str_hash((taida_val)(nm))); \
        taida_pack_set(req, f, (val)); \
        taida_pack_set_tag(req, f, (tag)); \
        f++; \
    } while(0)

    // D29B-011: method/path/query are span packs into req.raw on the
    // arena fast path. On OOM fallback they remain Str (legacy form).
    if (arena_size > 0) {
        SET_FIELD_H3("method", taida_net_make_span((taida_val)method_start, (taida_val)method_len), TAIDA_TAG_PACK);
        SET_FIELD_H3("path",   taida_net_make_span((taida_val)path_start,   (taida_val)path_part_len),   TAIDA_TAG_PACK);
        SET_FIELD_H3("query",  taida_net_make_span((taida_val)query_start,  (taida_val)query_part_len),  TAIDA_TAG_PACK);
    } else {
        SET_FIELD_H3("method", (taida_val)taida_str_new_copy(fields->method), TAIDA_TAG_STR);
        SET_FIELD_H3("path",   (taida_val)taida_str_new_copy(path_part),       TAIDA_TAG_STR);
        SET_FIELD_H3("query",  (taida_val)taida_str_new_copy(query_part),      TAIDA_TAG_STR);
    }
    SET_FIELD_H3("version",     version_pack,                                 TAIDA_TAG_PACK);
    SET_FIELD_H3("headers",     hdr_list,                                     TAIDA_TAG_LIST);
    // body span references the leading body_len bytes of the arena (offset 0)
    SET_FIELD_H3("body",        taida_net_make_span(0, (taida_val)body_len),  TAIDA_TAG_PACK);
    SET_FIELD_H3("bodyOffset",  (taida_val)0,                                 TAIDA_TAG_INT);
    SET_FIELD_H3("contentLength",(taida_val)(int64_t)body_len,                TAIDA_TAG_INT);
    // D29B-011: post-arena, body field is now a span pack (not a Bytes
    // ref), so raw_bytes is referenced exactly once via the "raw" field.
    // The previous taida_retain(raw_bytes) covered the dual-field shape;
    // with body now a span the extra retain would cause a leak.
    SET_FIELD_H3("raw",         raw_bytes,                                    TAIDA_TAG_PACK);
    SET_FIELD_H3("remoteHost",  (taida_val)taida_str_new_copy(peer_host),       TAIDA_TAG_STR);
    SET_FIELD_H3("remotePort",  (taida_val)(int64_t)peer_port,                TAIDA_TAG_INT);
    SET_FIELD_H3("keepAlive",   (taida_val)1,                                 TAIDA_TAG_BOOL);
    // HTTP/3 never uses chunked TE (binary framing like H2)
    SET_FIELD_H3("chunked",     (taida_val)0,                                 TAIDA_TAG_BOOL);
    SET_FIELD_H3("protocol",    (taida_val)taida_str_new_copy("h3"),            TAIDA_TAG_STR);
    #undef SET_FIELD_H3
    return req;
}

// ── H3 response send helpers ──────────────────────────────────────────────
// These build QPACK-encoded HEADERS frames and DATA frames for H3 responses.

// Build H3 HEADERS frame with QPACK-encoded response headers.
// Returns frame size, or -1 on error. Caller provides the output buffer.
static int h3_build_response_headers_frame(unsigned char *buf, size_t buf_cap,
                                            int status, const H3Header *headers, int header_count) {
    // NB7-34: 8192 bytes covers 99% of header blocks.
    // MTU 1200-65535; 8192 fits in a single QUIC packet payload (~4KB after MTU discovery).
    // Phase 6+: consider dynamic sizing based on SETTINGS max_field_section_size.
    unsigned char qpack_buf[8192];
    int qpack_len = h3_qpack_encode_block(qpack_buf, sizeof(qpack_buf),
                                           status, headers, header_count);
    if (qpack_len < 0) return -1;

    // Wrap in H3 HEADERS frame
    return h3_encode_frame(buf, buf_cap, H3_FRAME_HEADERS, qpack_buf, (size_t)qpack_len);
}

// Build H3 DATA frame.
// Returns frame size, or -1 on error.
static int h3_build_data_frame(unsigned char *buf, size_t buf_cap,
                                const unsigned char *data, size_t data_len) {
    return h3_encode_frame(buf, buf_cap, H3_FRAME_DATA, data, data_len);
}

// ── NET7-8a: libquiche dlopen FFI contract ────────────────────────────────
// Runtime loading of libquiche.so (shared library) — no compile-time headers
// needed. Follows the exact taida_ossl pattern at line ~7599.
//
// Opaque handle types — all quiche pointers are passed through without
// dereferencing at the C level.

typedef struct quiche_config quiche_config;
typedef struct quiche_conn quiche_conn;

// quiche constants
#define QUICHE_OK 0
#define QUICHE_H3_ALPN "\x02h3"

// NET7-8b: QUIC datagram size limit.
// QUIC long header max is ~32 bytes; remaining budget is the UDP payload.
// RFC 9000: initial_max_udp_payload_size is 65527.
#define QUICHE_MAX_DATAGRAM_SIZE 65527

// Function pointer table for the quiche symbols required for Phase 8
// (server-side QUIC transport + HTTP/3 dispatch).
static struct {
    int loaded;
    void *libquiche_handle;

    // quiche_config
    quiche_config *(*quiche_config_new)(const uint32_t version);
    void           (*quiche_config_free)(quiche_config *config);
    int            (*quiche_config_load_cert_chain_from_pem_file)(quiche_config *config, const char *path);
    int            (*quiche_config_load_priv_key_from_pem_file)(quiche_config *config, const char *path);
    int            (*quiche_config_set_application_protos)(quiche_config *config, const uint8_t *protos, size_t protos_len);
    void           (*quiche_config_verify_peer)(quiche_config *config, bool v);
    void           (*quiche_config_grease)(quiche_config *config, bool value);
    void           (*quiche_config_set_max_idle_timeout)(quiche_config *config, uint64_t v);

    // QUIC transport parameters (NET7-12c: required for stream data flow)
    void           (*quiche_config_set_initial_max_data)(quiche_config *config, uint64_t v);
    void           (*quiche_config_set_initial_max_stream_data_bidi_local)(quiche_config *config, uint64_t v);
    void           (*quiche_config_set_initial_max_stream_data_bidi_remote)(quiche_config *config, uint64_t v);
    void           (*quiche_config_set_initial_max_stream_data_uni)(quiche_config *config, uint64_t v);
    void           (*quiche_config_set_initial_max_streams_bidi)(quiche_config *config, uint64_t v);
    void           (*quiche_config_set_initial_max_streams_uni)(quiche_config *config, uint64_t v);

    // quiche_accept / connection lifecycle
    quiche_conn    *(*quiche_accept)(const uint8_t *dcid, size_t dcid_len,
                                     const uint8_t *odcid, size_t odcid_len,
                                     const quiche_config *config,
                                     struct sockaddr *addr, socklen_t addr_len);
    void            (*quiche_conn_free)(quiche_conn *conn);
    ssize_t         (*quiche_conn_recv)(quiche_conn *conn, uint8_t *buf, size_t buf_len,
                                        const struct sockaddr *from, socklen_t from_len);
    ssize_t         (*quiche_conn_send)(quiche_conn *conn, uint8_t *out, size_t out_len,
                                        struct sockaddr *to, socklen_t *to_len);
    bool            (*quiche_conn_is_established)(const quiche_conn *conn);
    bool            (*quiche_conn_is_closed)(const quiche_conn *conn);
    bool            (*quiche_conn_is_draining)(const quiche_conn *conn);
    int             (*quiche_conn_close)(quiche_conn *conn, int app, uint64_t err,
                                         const uint8_t *reason, size_t reason_len);
    bool            (*quiche_conn_is_in_early_data)(const quiche_conn *conn);
    // NET7-12d: Timer functions for drain wait (optional — graceful shutdown).
    uint64_t        (*quiche_conn_timeout_as_nanos)(const quiche_conn *conn);
    void            (*quiche_conn_on_timeout)(quiche_conn *conn);

    // stream send/recv
    int64_t         (*quiche_conn_stream_recv)(quiche_conn *conn, uint64_t stream_id,
                                               uint8_t *out, size_t buf_len, bool *fin);
    int64_t         (*quiche_conn_stream_send)(quiche_conn *conn, uint64_t stream_id,
                                               const uint8_t *buf, size_t buf_len, bool fin);
    int             (*quiche_conn_stream_shutdown)(quiche_conn *conn, uint64_t stream_id,
                                                  int direction, uint16_t app_error_code);

    // Stream iteration (NET7-12c: needed for H3 stream dispatch)
    void*           (*quiche_conn_readable)(const quiche_conn *conn);
    void*           (*quiche_conn_writable)(const quiche_conn *conn);
    int             (*quiche_stream_iter_next)(void *iter, uint64_t *stream_id);
    void            (*quiche_stream_iter_free)(void *iter);

    // version and accept helpers
    uint32_t        (*quiche_version)(void);
    int64_t         (*quiche_accept_dcid_len)(const uint8_t *buf, size_t buf_len);

    // header info / connection metadata
    // Note: dcid_len (input) and scid_len/token_len (output) are separate params
    int             (*quiche_header_info)(const uint8_t *buf, size_t buf_len,
                                          size_t dcid_len_input, uint32_t *version,
                                          uint8_t *type, uint8_t *dcid, size_t *dcid_output_len,
                                          uint8_t *scid, size_t *scid_output_len,
                                          uint8_t *token, size_t *token_output_len);

    // H3 layer (quiche-h3): HTTP/3 config and connection
    void*           (*quiche_h3_config_new)(void);
    void            (*quiche_h3_config_free)(void *config);
    void*           (*quiche_h3_conn_new_with_transport)(quiche_conn *quiche_conn, void *config);
    void            (*quiche_h3_conn_free)(void *h3_conn);

    // H3 polling and I/O
    ssize_t         (*quiche_h3_conn_poll)(void *h3_conn, uint64_t *stream_id, void *ev);
    ssize_t         (*quiche_h3_recv)(void *h3_conn, uint64_t stream_id,
                                      uint8_t *out, size_t out_len);
    ssize_t         (*quiche_h3_send)(void *h3_conn, quiche_conn *quiche_conn);
    ssize_t         (*quiche_h3_send_body)(void *h3_conn, quiche_conn *quiche_conn,
                                           uint64_t stream_id, uint8_t *body, size_t body_len, bool fin);

    // ── Optional symbols: loaded if present, NULL-checked before use. ──
    // Version negotiation (server-side retry)
    ssize_t         (*quiche_negotiate_version)(const uint8_t *scid, size_t scid_len,
                                                 const uint8_t *dcid, size_t dcid_len,
                                                 uint8_t *out, size_t out_len);
    ssize_t         (*quiche_retry)(const uint8_t *scid, size_t scid_len,
                                    const uint8_t *dcid, size_t dcid_len,
                                    const uint8_t *new_scid, size_t new_scid_len,
                                    const uint8_t *token, size_t token_len,
                                    uint32_t version, uint8_t *out, size_t out_len);
    // Stream priority
    int             (*quiche_conn_stream_priority)(quiche_conn *conn, uint64_t stream_id,
                                                    uint8_t urgency, int incremental);

} taida_quiche = { 0, NULL };

// Forward declaration.
static void taida_quiche_unload(void);

// Load libquiche and resolve all required symbols. Returns 1 on success, 0 on failure.
static int taida_quiche_load(void) {
    if (taida_quiche.loaded) return 1;

    // Try common shared library names.
    taida_quiche.libquiche_handle = dlopen("libquiche.so", RTLD_LAZY);
    if (!taida_quiche.libquiche_handle)
        taida_quiche.libquiche_handle = dlopen("libquiche.so.0", RTLD_LAZY);
    if (!taida_quiche.libquiche_handle) return 0;

    // Resolve symbols. Cast through void* to suppress -Wpedantic warnings.
    #define LOAD_QSYM(name) do { \
        *(void**)(&taida_quiche.name) = dlsym(taida_quiche.libquiche_handle, #name); \
        if (!taida_quiche.name) { taida_quiche_unload(); return 0; } \
    } while(0)

    // Config symbols (critical)
    LOAD_QSYM(quiche_config_new);
    LOAD_QSYM(quiche_config_free);
    LOAD_QSYM(quiche_config_load_cert_chain_from_pem_file);
    LOAD_QSYM(quiche_config_load_priv_key_from_pem_file);
    LOAD_QSYM(quiche_config_set_application_protos);
    LOAD_QSYM(quiche_config_verify_peer);
    LOAD_QSYM(quiche_config_grease);
    LOAD_QSYM(quiche_config_set_max_idle_timeout);

    // QUIC transport parameters (NET7-12c: required for stream data flow)
    LOAD_QSYM(quiche_config_set_initial_max_data);
    LOAD_QSYM(quiche_config_set_initial_max_stream_data_bidi_local);
    LOAD_QSYM(quiche_config_set_initial_max_stream_data_bidi_remote);
    LOAD_QSYM(quiche_config_set_initial_max_stream_data_uni);
    LOAD_QSYM(quiche_config_set_initial_max_streams_bidi);
    LOAD_QSYM(quiche_config_set_initial_max_streams_uni);

    // Connection lifecycle (critical)
    LOAD_QSYM(quiche_accept);
    LOAD_QSYM(quiche_accept_dcid_len);
    LOAD_QSYM(quiche_conn_free);
    LOAD_QSYM(quiche_conn_recv);
    LOAD_QSYM(quiche_conn_send);
    LOAD_QSYM(quiche_conn_is_established);
    LOAD_QSYM(quiche_conn_is_closed);
    LOAD_QSYM(quiche_conn_is_draining);
    LOAD_QSYM(quiche_conn_is_in_early_data);
    LOAD_QSYM(quiche_conn_close);

    // Stream I/O (critical)
    LOAD_QSYM(quiche_conn_stream_recv);
    LOAD_QSYM(quiche_conn_stream_send);
    LOAD_QSYM(quiche_conn_stream_shutdown);

    // Stream iteration (NET7-12c: critical for H3 dispatch)
    LOAD_QSYM(quiche_conn_readable);
    LOAD_QSYM(quiche_conn_writable);
    LOAD_QSYM(quiche_stream_iter_next);
    LOAD_QSYM(quiche_stream_iter_free);

    // Version info
    LOAD_QSYM(quiche_version);

    // Header info
    LOAD_QSYM(quiche_header_info);

    #undef LOAD_QSYM

    // ── Optional symbols: gracefully degrade if absent. ──
    // Phase 8a: H3 layer functions are optional — the QUIC transport
    // substrate (NET7-8a) only needs quiche_conn_* functions.
    // H3 framing (quiche_h3_*) is wired in Phase 8b/8c/8d.

    // H3 config / conn — used in Phase 8b for full QUIC+H3 integration
    *(void**)(&taida_quiche.quiche_h3_config_new) =
        dlsym(taida_quiche.libquiche_handle, "quiche_h3_config_new");
    *(void**)(&taida_quiche.quiche_h3_config_free) =
        dlsym(taida_quiche.libquiche_handle, "quiche_h3_config_free");
    *(void**)(&taida_quiche.quiche_h3_conn_new_with_transport) =
        dlsym(taida_quiche.libquiche_handle, "quiche_h3_conn_new_with_transport");
    *(void**)(&taida_quiche.quiche_h3_conn_free) =
        dlsym(taida_quiche.libquiche_handle, "quiche_h3_conn_free");
    *(void**)(&taida_quiche.quiche_h3_conn_poll) =
        dlsym(taida_quiche.libquiche_handle, "quiche_h3_conn_poll");
    *(void**)(&taida_quiche.quiche_h3_recv) =
        dlsym(taida_quiche.libquiche_handle, "quiche_h3_recv");
    *(void**)(&taida_quiche.quiche_h3_send) =
        dlsym(taida_quiche.libquiche_handle, "quiche_h3_send");
    *(void**)(&taida_quiche.quiche_h3_send_body) =
        dlsym(taida_quiche.libquiche_handle, "quiche_h3_send_body");

    // Version negotiation — only needed for server-side retry/version negotiation.
    *(void**)(&taida_quiche.quiche_negotiate_version) =
        dlsym(taida_quiche.libquiche_handle, "quiche_negotiate_version");
    *(void**)(&taida_quiche.quiche_retry) =
        dlsym(taida_quiche.libquiche_handle, "quiche_retry");

    // conn_stream_priority — useful for stream prioritization (optional)
    *(void**)(&taida_quiche.quiche_conn_stream_priority) =
        dlsym(taida_quiche.libquiche_handle, "quiche_conn_stream_priority");

    // NET7-12d: Timer functions for drain wait (optional).
    *(void**)(&taida_quiche.quiche_conn_timeout_as_nanos) =
        dlsym(taida_quiche.libquiche_handle, "quiche_conn_timeout_as_nanos");
    *(void**)(&taida_quiche.quiche_conn_on_timeout) =
        dlsym(taida_quiche.libquiche_handle, "quiche_conn_on_timeout");

    taida_quiche.loaded = 1;
    return 1;
}

static void taida_quiche_unload(void) {
    if (taida_quiche.libquiche_handle) {
        dlclose(taida_quiche.libquiche_handle);
        taida_quiche.libquiche_handle = NULL;
    }
    taida_quiche.loaded = 0;
}

// ── QPACK Encoder Instruction Stream (RFC 9204 Section 5.2) (NET7-10d) ───
// Encoder instructions for dynamic table management.
// Parity with Interpreter's encode_insert_with_name_ref, encode_insert_with_literal_name,
// encode_duplicate, encode_set_capacity, decode_encoder_instruction, apply_encoder_instruction.

typedef enum {
    H3_INST_NAME_REF,       // Insert With Name Reference (static or dynamic)
    H3_INST_LITERAL_NAME,   // Insert With Literal Name
    H3_INST_DUPLICATE,      // Duplicate
    H3_INST_SET_CAPACITY,   // Set Dynamic Table Capacity
} H3InstructionKind;

typedef struct {
    H3InstructionKind kind;
    int is_static;          // for NAME_REF
    uint64_t name_index;    // for NAME_REF / DUPLICATE
    uint64_t capacity;      // for SET_CAPACITY
    char name[128];         // for LITERAL_NAME / NAME_REF resolved
    char value[256];
} H3EncoderInstruction;

/// Encode Insert With Literal Name (RFC 9204 Section 5.2.2): 01xxxxxx
/// Returns bytes written, or -1 on error.
static int h3_qpack_encode_instruction_literal_name(unsigned char *buf, size_t buf_cap,
    const char *name, const char *value) {
    size_t pos = 0;
    size_t nlen = strlen(name);
    // Name length: 3-bit prefix, instruction byte 01 + N bits
    size_t niw;
    if (h3_qpack_encode_int(buf + pos, buf_cap - pos, 3, (uint64_t)nlen, 0x40, &niw) < 0) return -1;
    pos += niw;
    if (pos + nlen > buf_cap) return -1;
    memcpy(buf + pos, name, nlen);
    pos += nlen;
    // Value: 7-bit prefix string literal
    size_t vlen = strlen(value);
    size_t int_w;
    if (h3_qpack_encode_int(buf + pos, buf_cap - pos, 7, (uint64_t)vlen, 0x00, &int_w) < 0) return -1;
    pos += int_w;
    if (pos + vlen > buf_cap) return -1;
    memcpy(buf + pos, value, vlen);
    pos += vlen;
    return (int)pos;
}

/// Encode Duplicate (RFC 9204 Section 5.2.3): 00xxxxxx
static int h3_qpack_encode_instruction_duplicate(unsigned char *buf, size_t buf_cap, uint64_t index) {
    size_t w;
    if (h3_qpack_encode_int(buf, buf_cap, 6, index, 0x00, &w) < 0) return -1;
    return (int)w;
}

/// Encode Insert With Name Reference (static or dynamic)
static int h3_qpack_encode_instruction_name_ref(unsigned char *buf, size_t buf_cap,
    int is_static, uint64_t name_index, const char *value) {
    size_t pos = 0;
    uint8_t prefix = is_static ? 0xC0 : 0x80;
    size_t niw;
    if (h3_qpack_encode_int(buf + pos, buf_cap - pos, 4, name_index, prefix, &niw) < 0) return -1;
    pos += niw;
    size_t vlen = strlen(value);
    size_t int_w;
    if (h3_qpack_encode_int(buf + pos, buf_cap - pos, 7, (uint64_t)vlen, 0x00, &int_w) < 0) return -1;
    pos += int_w;
    if (pos + vlen > buf_cap) return -1;
    memcpy(buf + pos, value, vlen);
    pos += vlen;
    return (int)pos;
}

/// Encode Set Dynamic Table Capacity (RFC 9204 Section 5.2.4): 001xxxxx
static int h3_qpack_encode_instruction_set_capacity(unsigned char *buf, size_t buf_cap, uint64_t capacity) {
    size_t w;
    if (h3_qpack_encode_int(buf, buf_cap, 5, capacity, 0x20, &w) < 0) return -1;
    return (int)w;
}

/// Decode a single encoder instruction. Returns bytes consumed, or -1 on error.
static int h3_decode_encoder_instruction(const unsigned char *data, size_t data_len,
    H3EncoderInstruction *out) {
    if (data_len == 0) return -1;
    memset(out, 0, sizeof(*out));
    uint8_t byte = data[0];

    if (byte & 0x80) {
        // Insert With Name Reference: 1Txxxxxx (Section 5.2.1)
        out->is_static = (byte & 0x40) != 0;
        size_t ni_consumed;
        if (h3_qpack_decode_int(data, data_len, 4, &out->name_index, &ni_consumed) < 0) return -1;
        size_t val_consumed;
        if (h3_qpack_decode_string(data + ni_consumed, data_len - ni_consumed,
                                    out->value, sizeof(out->value), &val_consumed) < 0) return -1;
        out->kind = H3_INST_NAME_REF;
        return (int)(ni_consumed + val_consumed);
    } else if (byte & 0x40) {
        // Insert With Literal Name: 01xxxxxx (Section 5.2.2)
        uint64_t name_len;
        size_t nli_consumed;
        if (h3_qpack_decode_int(data, data_len, 3, &name_len, &nli_consumed) < 0) return -1;
        size_t offset = nli_consumed;
        if (offset + (size_t)name_len > data_len) return -1;
        if ((size_t)name_len >= sizeof(out->name)) return -1;
        memcpy(out->name, data + offset, (size_t)name_len);
        out->name[(size_t)name_len] = '\0';
        offset += (size_t)name_len;
        size_t val_consumed;
        if (h3_qpack_decode_string(data + offset, data_len - offset,
                                    out->value, sizeof(out->value), &val_consumed) < 0) return -1;
        out->kind = H3_INST_LITERAL_NAME;
        return (int)(offset + val_consumed);
    } else if (byte & 0x20) {
        // Set Dynamic Table Capacity: 001xxxxx (Section 5.2.4)
        size_t ci_consumed;
        if (h3_qpack_decode_int(data, data_len, 5, &out->capacity, &ci_consumed) < 0) return -1;
        out->kind = H3_INST_SET_CAPACITY;
        return (int)ci_consumed;
    } else {
        // Duplicate: 00xxxxxx (Section 5.2.3)
        size_t di_consumed;
        if (h3_qpack_decode_int(data, data_len, 6, &out->name_index, &di_consumed) < 0) return -1;
        out->kind = H3_INST_DUPLICATE;
        return (int)di_consumed;
    }
}

/// Apply an encoder instruction to a dynamic table.
/// (NET7-10d parity with Interpreter's apply_encoder_instruction)
static int h3_apply_encoder_instruction(H3DynamicTable *dt, const H3EncoderInstruction *inst) {
    switch (inst->kind) {
        case H3_INST_NAME_REF: {
            if (inst->is_static) {
                if (inst->name_index >= H3_QPACK_STATIC_TABLE_LEN) return 0;
                return h3_dt_insert(dt, H3_QPACK_STATIC_TABLE[inst->name_index].name, inst->value);
            } else {
                /* NB7-111 fix: name_index from the decoder is a relative index
                 * (0 = most recently inserted entry). Convert to absolute before
                 * lookup, matching RFC 9204 §5.2.1 semantics. */
                uint64_t abs_idx;
                if (!h3_dt_relative_to_absolute(dt, inst->name_index, &abs_idx)) return 0;
                const H3DynamicTableEntry *src = h3_dt_lookup_absolute(dt, abs_idx);
                if (!src) return 0;
                return h3_dt_insert(dt, src->name, inst->value);
            }
        }
        case H3_INST_LITERAL_NAME:
            return h3_dt_insert(dt, inst->name, inst->value);
        case H3_INST_DUPLICATE: {
            /* NB7-111 fix: index from the decoder is a relative index.
             * Convert to absolute before duplication, per RFC 9204 §5.2.3. */
            uint64_t abs_idx;
            if (!h3_dt_relative_to_absolute(dt, inst->name_index, &abs_idx)) return 0;
            return h3_dt_duplicate(dt, abs_idx);
        }
        case H3_INST_SET_CAPACITY:
            h3_dt_set_capacity(dt, (size_t)inst->capacity);
            return 1;
    }
    return 0;
}

// ── QPACK Decoder Instruction Stream (RFC 9204 Section 6.2) (NET7-10d) ───
// Decoder instructions sent from decoder to encoder.
// Parity with Interpreter's H3DecoderInstruction, decode_decoder_instruction, H3DecoderState.

typedef enum {
    H3_DEC_INST_SECTION_ACK,
    H3_DEC_INST_STREAM_CANCEL,
    H3_DEC_INST_INSERT_COUNT_INC,
} H3DecoderInstKind;

typedef struct {
    H3DecoderInstKind kind;
    uint64_t value; // insert_count (SECTION_ACK), stream_id (STREAM_CANCEL), increment (COUNT_INC)
} H3DecoderInstruction;

typedef struct {
    uint64_t received_insert_count;
    uint64_t acknowledged_insert_count;
} H3DecoderState;

static void h3_decoder_state_init(H3DecoderState *state) {
    state->received_insert_count = 0;
    state->acknowledged_insert_count = 0;
}

static int h3_decode_decoder_instruction(const unsigned char *data, size_t data_len,
    H3DecoderInstruction *out) {
    if (data_len == 0) return -1;
    uint8_t byte = data[0];
    if (byte & 0x80) {
        // Section Ack: 1xxxxxxx (7-bit prefix)
        size_t c;
        if (h3_qpack_decode_int(data, data_len, 7, &out->value, &c) < 0) return -1;
        out->kind = H3_DEC_INST_SECTION_ACK;
        return (int)c;
    } else if (byte & 0x20) {
        // Stream Cancel: 001xxxxx (5-bit prefix)
        size_t c;
        if (h3_qpack_decode_int(data, data_len, 5, &out->value, &c) < 0) return -1;
        out->kind = H3_DEC_INST_STREAM_CANCEL;
        return (int)c;
    } else {
        // Insert Count Increment: 00xxxxxx (6-bit prefix)
        size_t c;
        if (h3_qpack_decode_int(data, data_len, 6, &out->value, &c) < 0) return -1;
        out->kind = H3_DEC_INST_INSERT_COUNT_INC;
        return (int)c;
    }
}

static int h3_decoder_apply(H3DecoderState *state, const H3DecoderInstruction *inst) {
    switch (inst->kind) {
        case H3_DEC_INST_SECTION_ACK:
            if (inst->value > state->acknowledged_insert_count)
                state->acknowledged_insert_count = inst->value;
            return 1;
        case H3_DEC_INST_STREAM_CANCEL:
            return 1; // no-op in simplified model
        case H3_DEC_INST_INSERT_COUNT_INC:
            if (inst->value == 0) return 0; // zero increment is illegal (RFC 9204 §6.2.3)
            /* NB7-113 fix: use saturated addition to match Interpreter's
             * checked_add(...).unwrap_or(u64::MAX) behavior. This prevents
             * wrap-around on overflow, which would corrupt decoder state. */
            if (inst->value > UINT64_MAX - state->received_insert_count) {
                state->received_insert_count = UINT64_MAX;
            } else {
                state->received_insert_count += inst->value;
            }
            return 1;
    }
    return 0;
}

// ── H3 self-tests (NB7-9, NB7-10) ────────────────────────────────────────
// Embedded self-tests for QPACK round-trip and H3 request validation.
// Called from taida_net_h3_serve() to ensure Phase 2 reference semantics
// are correct before entering the QUIC transport layer.

// NB7-9: QPACK encode/decode round-trip self-test.
// Verifies that headers with literal names (not in static table) survive
// a full encode → decode cycle.
static int h3_selftest_qpack_roundtrip(void) {
    H3Header input[4];
    memset(input, 0, sizeof(input));
    // Header 0: static table hit (:status 200 uses indexed field line)
    // We test with regular headers only for the round-trip
    snprintf(input[0].name, sizeof(input[0].name), "content-type");
    snprintf(input[0].value, sizeof(input[0].value), "text/plain");
    // Header 1: literal name NOT in static table
    snprintf(input[1].name, sizeof(input[1].name), "x-custom-header");
    snprintf(input[1].value, sizeof(input[1].value), "custom-value-123");
    // Header 2: another literal name
    snprintf(input[2].name, sizeof(input[2].name), "x-request-id");
    snprintf(input[2].value, sizeof(input[2].value), "abc-def-ghi");
    // Header 3: static table name match with custom value
    snprintf(input[3].name, sizeof(input[3].name), "accept");
    snprintf(input[3].value, sizeof(input[3].value), "application/json");

    // Encode
    unsigned char buf[4096];
    int enc_len = h3_qpack_encode_block(buf, sizeof(buf), 200, input, 4);
    if (enc_len < 0) return -1; // encode failed

    // Decode
    H3Header output[8];
    int dec_count = h3_qpack_decode_block(buf, (size_t)enc_len, output, 8);
    // Expected: :status + 4 headers = 5
    if (dec_count != 5) return -2; // header count mismatch

    // Verify :status
    if (strcmp(output[0].name, ":status") != 0) return -3;
    if (strcmp(output[0].value, "200") != 0) return -4;

    // Verify round-trip for each input header
    // Note: the encoder outputs :status first, then the custom headers
    for (int i = 0; i < 4; i++) {
        if (strcmp(output[i + 1].name, input[i].name) != 0) return -(10 + i);
        if (strcmp(output[i + 1].value, input[i].value) != 0) return -(20 + i);
    }

    // NB7-11: Test max_headers overflow: encode 5 fields (:status + 4 headers)
    // but decode with max=2. Must return -1 (decode error), matching H2 behavior.
    // Before NB7-11 fix, this returned partial count (silent truncation).
    int overflow_count = h3_qpack_decode_block(buf, (size_t)enc_len, output, 2);
    if (overflow_count != -1) return -30; // overflow must be decode error (H2 parity)

    return 0; // all tests passed
}

// NB7-10: H3 request pseudo-header validation self-test.
// Verifies that :scheme is required, empty values are rejected,
// and validation matches H2 semantics.
static int h3_selftest_request_validation(void) {
    // Test 1: Valid request with all required pseudo-headers
    {
        H3Header hdrs[4];
        memset(hdrs, 0, sizeof(hdrs));
        snprintf(hdrs[0].name, sizeof(hdrs[0].name), ":method");
        snprintf(hdrs[0].value, sizeof(hdrs[0].value), "GET");
        snprintf(hdrs[1].name, sizeof(hdrs[1].name), ":path");
        snprintf(hdrs[1].value, sizeof(hdrs[1].value), "/");
        snprintf(hdrs[2].name, sizeof(hdrs[2].name), ":scheme");
        snprintf(hdrs[2].value, sizeof(hdrs[2].value), "https");
        snprintf(hdrs[3].name, sizeof(hdrs[3].name), ":authority");
        snprintf(hdrs[3].value, sizeof(hdrs[3].value), "localhost");
        H3RequestFields out;
        h3_extract_request_fields(hdrs, 4, &out);
        if (!out.ok) return -1; // valid request should succeed
        if (out.regular_headers) free(out.regular_headers);
    }

    // Test 2: Missing :scheme should fail (NB7-10 fix)
    {
        H3Header hdrs[2];
        memset(hdrs, 0, sizeof(hdrs));
        snprintf(hdrs[0].name, sizeof(hdrs[0].name), ":method");
        snprintf(hdrs[0].value, sizeof(hdrs[0].value), "GET");
        snprintf(hdrs[1].name, sizeof(hdrs[1].name), ":path");
        snprintf(hdrs[1].value, sizeof(hdrs[1].value), "/");
        H3RequestFields out;
        h3_extract_request_fields(hdrs, 2, &out);
        if (out.ok) return -2; // missing :scheme should fail
        if (out.error_reason != H3_REQ_ERR_MISSING_PSEUDO) return -3;
    }

    // Test 3: Empty :scheme value should fail
    {
        H3Header hdrs[3];
        memset(hdrs, 0, sizeof(hdrs));
        snprintf(hdrs[0].name, sizeof(hdrs[0].name), ":method");
        snprintf(hdrs[0].value, sizeof(hdrs[0].value), "GET");
        snprintf(hdrs[1].name, sizeof(hdrs[1].name), ":path");
        snprintf(hdrs[1].value, sizeof(hdrs[1].value), "/");
        snprintf(hdrs[2].name, sizeof(hdrs[2].name), ":scheme");
        hdrs[2].value[0] = '\0'; // empty value
        H3RequestFields out;
        h3_extract_request_fields(hdrs, 3, &out);
        if (out.ok) return -4; // empty :scheme should fail
        if (out.error_reason != H3_REQ_ERR_EMPTY_PSEUDO) return -5;
    }

    // Test 4: Empty :method value should fail
    {
        H3Header hdrs[3];
        memset(hdrs, 0, sizeof(hdrs));
        snprintf(hdrs[0].name, sizeof(hdrs[0].name), ":method");
        hdrs[0].value[0] = '\0'; // empty
        snprintf(hdrs[1].name, sizeof(hdrs[1].name), ":path");
        snprintf(hdrs[1].value, sizeof(hdrs[1].value), "/");
        snprintf(hdrs[2].name, sizeof(hdrs[2].name), ":scheme");
        snprintf(hdrs[2].value, sizeof(hdrs[2].value), "https");
        H3RequestFields out;
        h3_extract_request_fields(hdrs, 3, &out);
        if (out.ok) return -6; // empty :method should fail
        if (out.error_reason != H3_REQ_ERR_EMPTY_PSEUDO) return -7;
    }

    // Test 5: Duplicate :scheme should fail
    {
        H3Header hdrs[4];
        memset(hdrs, 0, sizeof(hdrs));
        snprintf(hdrs[0].name, sizeof(hdrs[0].name), ":method");
        snprintf(hdrs[0].value, sizeof(hdrs[0].value), "GET");
        snprintf(hdrs[1].name, sizeof(hdrs[1].name), ":path");
        snprintf(hdrs[1].value, sizeof(hdrs[1].value), "/");
        snprintf(hdrs[2].name, sizeof(hdrs[2].name), ":scheme");
        snprintf(hdrs[2].value, sizeof(hdrs[2].value), "https");
        snprintf(hdrs[3].name, sizeof(hdrs[3].name), ":scheme");
        snprintf(hdrs[3].value, sizeof(hdrs[3].value), "http");
        H3RequestFields out;
        h3_extract_request_fields(hdrs, 4, &out);
        if (out.ok) return -8; // duplicate :scheme should fail
        if (out.error_reason != H3_REQ_ERR_DUPLICATE_PSEUDO) return -9;
    }

    // Test 6: Ordering violation (regular before pseudo)
    {
        H3Header hdrs[3];
        memset(hdrs, 0, sizeof(hdrs));
        snprintf(hdrs[0].name, sizeof(hdrs[0].name), "host");
        snprintf(hdrs[0].value, sizeof(hdrs[0].value), "localhost");
        snprintf(hdrs[1].name, sizeof(hdrs[1].name), ":method");
        snprintf(hdrs[1].value, sizeof(hdrs[1].value), "GET");
        snprintf(hdrs[2].name, sizeof(hdrs[2].name), ":path");
        snprintf(hdrs[2].value, sizeof(hdrs[2].value), "/");
        H3RequestFields out;
        h3_extract_request_fields(hdrs, 3, &out);
        if (out.ok) return -10; // ordering violation should fail
        if (out.error_reason != H3_REQ_ERR_ORDERING) return -11;
    }

    // Test 7: Unknown pseudo-header should fail
    {
        H3Header hdrs[4];
        memset(hdrs, 0, sizeof(hdrs));
        snprintf(hdrs[0].name, sizeof(hdrs[0].name), ":method");
        snprintf(hdrs[0].value, sizeof(hdrs[0].value), "GET");
        snprintf(hdrs[1].name, sizeof(hdrs[1].name), ":path");
        snprintf(hdrs[1].value, sizeof(hdrs[1].value), "/");
        snprintf(hdrs[2].name, sizeof(hdrs[2].name), ":scheme");
        snprintf(hdrs[2].value, sizeof(hdrs[2].value), "https");
        snprintf(hdrs[3].name, sizeof(hdrs[3].name), ":protocol");
        snprintf(hdrs[3].value, sizeof(hdrs[3].value), "websocket");
        H3RequestFields out;
        h3_extract_request_fields(hdrs, 4, &out);
        if (out.ok) return -12; // unknown pseudo should fail
        if (out.error_reason != H3_REQ_ERR_UNKNOWN_PSEUDO) return -13;
    }

    return 0; // all tests passed
}

// NET7-10d: QPACK dynamic table self-test (parity with Interpreter H3DynamicTable).
// Verifies insert, lookup (absolute/post-base), eviction, duplicate,
// set_capacity, relative_to_absolute, and instruction encode/decode.
static int h3_selftest_qpack_dynamic_table(void) {
    // Test 1: Insert and lookup_absolute
    {
        H3DynamicTable dt;
        h3_dynamic_table_init(&dt, 4096);
        if (h3_dt_len(&dt) != 0) return -1;
        if (!h3_dt_insert(&dt, "content-type", "text/html")) return -2;
        if (h3_dt_len(&dt) != 1) return -3;

        const H3DynamicTableEntry *e = h3_dt_lookup_absolute(&dt, 0);
        if (!e) return -4;
        if (strcmp(e->name, "content-type") != 0) return -5;
        if (strcmp(e->value, "text/html") != 0) return -6;
    }

    // Test 2: Eviction
    {
        H3DynamicTable dt;
        h3_dynamic_table_init(&dt, 100);
        // Each entry: name(5) + value(7) + 32 = 44 bytes
        if (!h3_dt_insert(&dt, "x-key", "value-1")) return -10;
        // 6 + 7 + 32 = 45 bytes
        if (!h3_dt_insert(&dt, "x-key2", "value-2")) return -11;
        if (h3_dt_len(&dt) != 2) return -12;

        // Third entry: 5 + 4 + 32 = 41 bytes. Total would be 44+45+41=130 > 100
        if (!h3_dt_insert(&dt, "name3", "val3")) return -13;
        if (h3_dt_len(&dt) > 2) return -14;
        // First entry (index 0) should be evicted
        if (h3_dt_lookup_absolute(&dt, 0) != NULL) return -15;
    }

    // Test 3: Insertion that alone exceeds capacity
    {
        H3DynamicTable dt;
        h3_dynamic_table_init(&dt, 50);
        if (h3_dt_insert(&dt, "very-long-name-here", "very-long-value-here")) return -20;
    }

    // Test 4: Post-base lookup
    {
        H3DynamicTable dt;
        h3_dynamic_table_init(&dt, 4096);
        h3_dt_insert(&dt, "a", "1");
        h3_dt_insert(&dt, "b", "2");
        h3_dt_insert(&dt, "c", "3");

        // post-base 0 = most recent (c, index 2)
        const H3DynamicTableEntry *e0 = h3_dt_lookup_post_base(&dt, 0);
        if (!e0 || e0->index != 2) return -30;
        // post-base 2 = oldest (a, index 0)
        const H3DynamicTableEntry *e2 = h3_dt_lookup_post_base(&dt, 2);
        if (!e2 || e2->index != 0) return -31;
        // post-base 3 (beyond total_inserted=3) should return NULL
        if (h3_dt_lookup_post_base(&dt, 3) != NULL) return -32;
    }

    // Test 5: Duplicate
    {
        H3DynamicTable dt;
        h3_dynamic_table_init(&dt, 4096);
        h3_dt_insert(&dt, "original", "data"); // index 0
        if (!h3_dt_duplicate(&dt, 0)) return -40;
        if (h3_dt_len(&dt) != 2) return -41;
        const H3DynamicTableEntry *dup = h3_dt_lookup_absolute(&dt, 1);
        if (!dup) return -42;
        if (strcmp(dup->name, "original") != 0) return -43;
        if (strcmp(dup->value, "data") != 0) return -44;
        // Duplicate non-existent should fail
        if (h3_dt_duplicate(&dt, 99)) return -45;
    }

    // Test 6: Set capacity shrink with eviction
    {
        H3DynamicTable dt;
        h3_dynamic_table_init(&dt, 200);
        h3_dt_insert(&dt, "name1", "val1"); // 5+4+32=41
        h3_dt_insert(&dt, "name2", "val2"); // 41
        h3_dt_insert(&dt, "name3", "val3"); // 41, total=123
        if (h3_dt_len(&dt) != 3) return -50;

        h3_dt_set_capacity(&dt, 80); // can hold only 1 entry
        if (h3_dt_len(&dt) != 1) return -51;
        if (h3_dt_capacity(&dt) != 80) return -52;
        // Only newest (index 2) should remain
        if (h3_dt_lookup_absolute(&dt, 2) == NULL) return -53;
    }

    // Test 7: Relative to absolute
    {
        H3DynamicTable dt;
        h3_dynamic_table_init(&dt, 4096);
        h3_dt_insert(&dt, "a", "1");
        h3_dt_insert(&dt, "b", "2");
        h3_dt_insert(&dt, "c", "3");

        uint64_t abs;
        if (!h3_dt_relative_to_absolute(&dt, 0, &abs) || abs != 2) return -60;
        if (!h3_dt_relative_to_absolute(&dt, 1, &abs) || abs != 1) return -61;
        if (!h3_dt_relative_to_absolute(&dt, 2, &abs) || abs != 0) return -62;
        if (h3_dt_relative_to_absolute(&dt, 3, &abs)) return -63;
    }

    // Test 8: Monotonic indices after eviction
    {
        H3DynamicTable dt;
        h3_dynamic_table_init(&dt, 68);
        // Each entry: 7+2+32=41 bytes; two entries=82>68
        h3_dt_insert(&dt, "name-01", "vv"); // index 0
        if (h3_dt_len(&dt) != 1) return -70;
        h3_dt_insert(&dt, "name-02", "vv"); // index 1, evicts 0
        if (h3_dt_len(&dt) != 1) return -71;
        if (h3_dt_lookup_absolute(&dt, 1) == NULL) return -72;
        if (h3_dt_lookup_absolute(&dt, 0) != NULL) return -73;
        if (h3_dt_total_inserted(&dt) != 2) return -74;
        if (h3_dt_largest_ref(&dt) != 1) return -75;
    }

    // Test 9: Encoder instruction encode/decode round-trip
    // Insert With Literal Name
    {
        unsigned char buf[64];
        int w = h3_qpack_encode_instruction_literal_name(buf, sizeof(buf), "x-custom", "hello");
        if (w < 0) return -80;
        // Verify first byte is 01xxxxxx
        if ((buf[0] >> 6) != 0b01) return -81;

        H3EncoderInstruction inst;
        int consumed = h3_decode_encoder_instruction(buf, (size_t)w, &inst);
        if (consumed != w) return -82;
        if (inst.kind != H3_INST_LITERAL_NAME) return -83;
        if (strcmp(inst.name, "x-custom") != 0) return -84;
        if (strcmp(inst.value, "hello") != 0) return -85;
    }

    // Test 10: Encoder instruction sequence + apply
    {
        H3DynamicTable dt;
        h3_dynamic_table_init(&dt, 4096);

        unsigned char buf[128];
        int pos = 0;

        // Insert With Literal Name
        int w = h3_qpack_encode_instruction_literal_name(buf + pos, sizeof(buf) - (size_t)pos, "x-a", "1");
        if (w < 0) return -90;
        pos += w;

        // Duplicate (index 0)
        w = h3_qpack_encode_instruction_duplicate(buf + pos, sizeof(buf) - (size_t)pos, 0);
        if (w < 0) return -91;
        pos += w;

        int offset = 0;
        while (offset < pos) {
            H3EncoderInstruction inst;
            int c = h3_decode_encoder_instruction(buf + (size_t)offset, (size_t)(pos - offset), &inst);
            if (c <= 0) return -(92 + offset);
            if (!h3_apply_encoder_instruction(&dt, &inst)) return -95;
            offset += c;
        }
        if (h3_dt_len(&dt) != 2) return -96;
    }

    return 0;
}

// Combined self-test runner. Returns 0 on success, or a diagnostic code.
static int h3_run_selftests(void) {
    int rc;
    rc = h3_selftest_qpack_roundtrip();
    if (rc != 0) return 1000 + (-rc); // 1001..1030 = QPACK failures
    rc = h3_selftest_request_validation();
    if (rc != 0) return 2000 + (-rc); // 2001..2013 = validation failures
    rc = h3_selftest_qpack_dynamic_table();
    if (rc != 0) return 3000 + (-rc); // 3001..3100 = dynamic table failures
    return 0;
}

// ── NET7-8b: QUIC connection pool ─────────────────────────────────────────
// Bounded connection pool for the UDP-based QUIC accept loop.
// Unlike TCP (which uses thread pools with client fds), QUIC/UDP uses a
// single socket where each packet is demultiplexed by DCID to a connection.
//
// bounded-copy discipline: 1 packet = at most 1 materialization.
// No aggregate buffer above packet boundary.

// H3ServeResult — return type for taida_net_h3_serve and serve_h3_loop.
// Defined here (before the pool and loop) so serve_h3_loop can use it.
typedef struct { int64_t requests; } H3ServeResult;

#define QUIC_MAX_CONNECTIONS 256

typedef struct {
    quiche_conn   *conn;             // opaque QUIC connection (FFI handle)
    struct sockaddr_in peer_addr;    // peer address for sendto()
    uint64_t         dcid_hash;      // hash of DCID for fast packet routing
    int64_t          conn_id;        // unique connection id (0-based index)
    int              active;         // 0 = free slot, 1 = active
    int              established;    // 0 = handshake pending, 1 = established (ALPN OK)
    // NET7-12c: Per-connection H3 protocol state
    H3Conn           h3_conn;        // H3 frame/stream/QPACK state
    int              h3_initialized; // 0 = needs init, 1 = control stream sent
    int              ctrl_stream_created; // 0 = not yet, 1 = control stream open
    uint64_t         ctrl_stream_id; // server-initiated unidirectional control stream
    // NET7-12d: Draining state for graceful shutdown.
    // 0 = normal, 1 = GOAWAY sent, waiting for drain completion.
    int              draining;
} QuicConnSlot;

typedef struct {
    QuicConnSlot  slots[QUIC_MAX_CONNECTIONS];
    int            count;           // active connection count
    int            max_connections;
    pthread_mutex_t mutex;
    int64_t        request_count;
    int64_t        max_requests;
    int            shutdown;        // flag: 1 = shutting down
    taida_val      handler;
    int64_t        timeout_ms;
    int            handler_arity;
    const char    *cert_path;
    const char    *key_path;
} QuicConnPool;

static void quic_pool_init(QuicConnPool *pool, int max_conn, taida_val handler,
                           int64_t max_requests, int64_t timeout_ms, int handler_arity,
                           const char *cert_path, const char *key_path) {
    pthread_mutex_init(&pool->mutex, NULL);
    pool->count = 0;
    pool->max_connections = max_conn;
    pool->request_count = 0;
    pool->max_requests = max_requests;
    pool->shutdown = 0;
    pool->handler = handler;
    pool->timeout_ms = timeout_ms;
    pool->handler_arity = handler_arity;
    pool->cert_path = cert_path;
    pool->key_path = key_path;
    for (int i = 0; i < QUIC_MAX_CONNECTIONS; i++) {
        pool->slots[i].conn = NULL;
        pool->slots[i].active = 0;
        pool->slots[i].conn_id = -1;
        pool->slots[i].dcid_hash = 0;
        pool->slots[i].established = 0;
        pool->slots[i].h3_initialized = 0;
        pool->slots[i].ctrl_stream_created = 0;
        pool->slots[i].ctrl_stream_id = 0;
        pool->slots[i].draining = 0;
        h3_conn_init(&pool->slots[i].h3_conn);
    }
}

// NET7-8c: FNV-1a 64-bit hash for DCID-based connection routing.
// Simple, fast, and deterministic — no dependency on external hash libs.
static uint64_t _fnv1a_64(const uint8_t *data, size_t len) {
    uint64_t hash = 14695981039346656037ULL; // FNV offset basis
    for (size_t i = 0; i < len; i++) {
        hash ^= (uint64_t)data[i];
        hash *= 1099511628211ULL; // FNV prime
    }
    return hash;
}

// NET7-8c: Lookup connection slot by DCID hash.
// Returns slot index (>=0) if found, -1 otherwise.
static int quic_pool_find_by_dcid(QuicConnPool *pool, uint64_t dcid_hash) {
    pthread_mutex_lock(&pool->mutex);
    for (int i = 0; i < QUIC_MAX_CONNECTIONS; i++) {
        if (pool->slots[i].active && pool->slots[i].dcid_hash == dcid_hash) {
            pthread_mutex_unlock(&pool->mutex);
            return i;
        }
    }
    pthread_mutex_unlock(&pool->mutex);
    return -1;
}

// NET7-8c: Connection maintenance pass.
// Closes connections that are fully closed or draining.
// Called periodically during the I/O event loop.
// NB7-74: scans only active slots (pool->count), not all 256 slots.
// Early-exits once all active connections have been checked.
static void h3_conn_maintenance(QuicConnPool *pool) {
    pthread_mutex_lock(&pool->mutex);
    int remaining = pool->count;
    for (int i = 0; i < QUIC_MAX_CONNECTIONS && remaining > 0; i++) {
        if (!pool->slots[i].active) continue;
        remaining--;
        if (!pool->slots[i].conn) continue;

        // Check closed/draining state.
        if (taida_quiche.quiche_conn_is_closed(pool->slots[i].conn) ||
            taida_quiche.quiche_conn_is_draining(pool->slots[i].conn)) {
            taida_quiche.quiche_conn_free(pool->slots[i].conn);
            h3_conn_free(&pool->slots[i].h3_conn);
            pool->slots[i].conn = NULL;
            pool->slots[i].active = 0;
            pool->slots[i].conn_id = -1;
            pool->slots[i].dcid_hash = 0;
            pool->slots[i].established = 0;
            pool->slots[i].h3_initialized = 0;
            pool->slots[i].ctrl_stream_created = 0;
            pool->count--;
        }
    }
    pthread_mutex_unlock(&pool->mutex);
}

// Find or create a slot for a connection identified by its DCID hash.
// Returns slot index, or -1 if pool is full.
static int quic_pool_find_or_create(QuicConnPool *pool, quiche_conn *conn,
                                     const struct sockaddr_in *peer,
                                     uint64_t dcid_hash) {
    pthread_mutex_lock(&pool->mutex);

    // Find a free slot.
    int free_slot = -1;
    for (int i = 0; i < QUIC_MAX_CONNECTIONS; i++) {
        if (!pool->slots[i].active) {
            free_slot = i;
            break;
        }
    }

    if (free_slot < 0) {
        // Pool is full — bounded rejection, no allocation.
        pthread_mutex_unlock(&pool->mutex);
        return -1;
    }

    pool->slots[free_slot].conn = conn;
    pool->slots[free_slot].peer_addr = *peer;
    pool->slots[free_slot].conn_id = (int64_t)free_slot;
    pool->slots[free_slot].dcid_hash = dcid_hash;
    pool->slots[free_slot].active = 1;
    pool->slots[free_slot].established = 0;
    pool->count++;

    pthread_mutex_unlock(&pool->mutex);
    return free_slot;
}

static void quic_pool_close_slot(QuicConnPool *pool, int slot_idx) {
    pthread_mutex_lock(&pool->mutex);
    if (slot_idx >= 0 && slot_idx < QUIC_MAX_CONNECTIONS && pool->slots[slot_idx].active) {
        if (pool->slots[slot_idx].conn && taida_quiche.quiche_conn_free) {
            taida_quiche.quiche_conn_free(pool->slots[slot_idx].conn);
        }
        h3_conn_free(&pool->slots[slot_idx].h3_conn);
        pool->slots[slot_idx].conn = NULL;
        pool->slots[slot_idx].active = 0;
        pool->slots[slot_idx].conn_id = -1;
        pool->slots[slot_idx].dcid_hash = 0;
        pool->slots[slot_idx].established = 0;
        pool->slots[slot_idx].h3_initialized = 0;
        pool->slots[slot_idx].ctrl_stream_created = 0;
        pool->slots[slot_idx].draining = 0;
        pool->count--;
    }
    pthread_mutex_unlock(&pool->mutex);
}

static void quic_pool_destroy(QuicConnPool *pool) {
    // Close all remaining connections and free per-connection H3 state.
    for (int i = 0; i < QUIC_MAX_CONNECTIONS; i++) {
        if (pool->slots[i].active) {
            h3_conn_free(&pool->slots[i].h3_conn);
            if (pool->slots[i].conn) {
                taida_quiche.quiche_conn_free(pool->slots[i].conn);
            }
        }
    }
    pthread_mutex_destroy(&pool->mutex);
}

// Check if connection count is exhausted (matching h1/h2 pattern).
static int quic_pool_requests_exhausted(QuicConnPool *pool) {
    return (pool->max_requests > 0 && pool->request_count >= pool->max_requests) ? 1 : 0;
}

// ── NET7-12c: Drain all pending outbound QUIC datagrams for a connection. ──
// quiche_conn_send() may produce multiple datagrams; we must drain them all.
// Returns 0 on success, -1 on fatal send error.
static int quic_drain_send(int udp_fd, quiche_conn *conn,
                           unsigned char *send_buf, size_t send_buf_cap) {
    for (;;) {
        struct sockaddr_in send_addr;
        socklen_t send_addr_len = sizeof(send_addr);
        ssize_t send_rc = taida_quiche.quiche_conn_send(
            conn, send_buf, send_buf_cap,
            (struct sockaddr*)&send_addr, &send_addr_len
        );
        if (send_rc < 0) {
            // QUICHE_ERR_DONE (-2) or other: no more data to send.
            break;
        }
        if (send_rc == 0) break;
        ssize_t n = sendto(udp_fd, send_buf, (size_t)send_rc, 0,
                           (struct sockaddr*)&send_addr, send_addr_len);
        if (n < 0) return -1;
    }
    return 0;
}

// ── NET7-12c: Initialize H3 control stream for a newly established connection. ──
// Sends a server-initiated unidirectional control stream with SETTINGS frame
// (RFC 9114 Section 3.2, Section 6.2.1).
// Returns 0 on success, -1 on error.
static int h3_init_control_stream(QuicConnSlot *slot) {
    if (slot->h3_initialized) return 0;

    // Server-initiated unidirectional stream IDs have form 4*N + 3 in QUIC.
    // Stream ID 3 = first server-initiated unidirectional stream.
    uint64_t ctrl_sid = 3;

    // Send stream type byte (0x00 = control stream, RFC 9114 Section 6.2).
    unsigned char stream_type = 0x00;
    int64_t wrc = taida_quiche.quiche_conn_stream_send(
        slot->conn, ctrl_sid, &stream_type, 1, 0 /*fin=false*/);
    if (wrc < 0) return -1;

    // Encode and send SETTINGS frame.
    unsigned char settings_payload[64];
    int settings_len = h3_encode_settings(settings_payload, sizeof(settings_payload));
    if (settings_len < 0) return -1;

    unsigned char settings_frame[128];
    int frame_len = h3_encode_frame(settings_frame, sizeof(settings_frame),
                                     H3_FRAME_SETTINGS, settings_payload, (size_t)settings_len);
    if (frame_len < 0) return -1;

    wrc = taida_quiche.quiche_conn_stream_send(
        slot->conn, ctrl_sid, settings_frame, (size_t)frame_len, 0 /*fin=false*/);
    if (wrc < 0) return -1;

    slot->ctrl_stream_id = ctrl_sid;
    slot->ctrl_stream_created = 1;
    slot->h3_initialized = 1;
    h3_conn_init(&slot->h3_conn);
    return 0;
}

// ── NET7-12c: Process a single readable QUIC stream (H3 dispatch). ──
//
// Responsibilities:
//   - Control stream (client-initiated unidirectional, stream_id & 0x03 == 0x02):
//     Read and decode SETTINGS / GOAWAY frames.
//   - Request stream (client-initiated bidirectional, stream_id & 0x03 == 0x00):
//     Read H3 frames, QPACK decode HEADERS, build request pack,
//     dispatch handler via taida_invoke_callback1(), encode response, send back.
//
// Returns: 1 = valid request served (increment pool.request_count)
//          0 = no request (control stream, error, incomplete data)
//         -1 = fatal connection error (caller should close slot)
static int h3_process_stream(QuicConnSlot *slot, QuicConnPool *pool,
                              uint64_t stream_id) {
    // Determine stream type from 2 LSBs of stream ID (RFC 9000 Section 2.1).
    // 0x0 = client-initiated bidirectional (request streams)
    // 0x2 = client-initiated unidirectional (control/QPACK streams)
    int stream_type = (int)(stream_id & 0x03);

    // Read stream data into a bounded buffer.
    // bounded-copy discipline: single materialization per stream read.
    unsigned char stream_buf[65536]; // 64KB — bounded by max_field_section_size
    size_t total_read = 0;
    bool fin = false;

    for (;;) {
        bool chunk_fin = false;
        int64_t rrc = taida_quiche.quiche_conn_stream_recv(
            slot->conn, stream_id,
            stream_buf + total_read, sizeof(stream_buf) - total_read,
            &chunk_fin
        );
        if (rrc < 0) break; // QUICHE_ERR_DONE or error
        total_read += (size_t)rrc;
        if (chunk_fin) { fin = true; break; }
        if (total_read >= sizeof(stream_buf)) break; // buffer full
    }

    if (total_read == 0 && !fin) return 0; // no data yet

    // ── Client-initiated unidirectional stream (control/QPACK) ──
    if (stream_type == 0x02) {
        if (total_read < 1) return 0;
        uint8_t uni_type = stream_buf[0];

        if (uni_type == 0x00) {
            // Control stream: decode frames (SETTINGS, GOAWAY).
            size_t pos = 1;
            while (pos < total_read) {
                uint64_t frame_type, frame_length;
                size_t header_size;
                if (h3_decode_frame_header(stream_buf + pos, total_read - pos,
                                            &frame_type, &frame_length, &header_size) < 0) {
                    break;
                }
                const unsigned char *payload = stream_buf + pos + header_size;
                size_t payload_len = (size_t)frame_length;
                pos += header_size + payload_len;

                if (frame_type == H3_FRAME_SETTINGS) {
                    h3_decode_settings(&slot->h3_conn, payload, payload_len);
                } else if (frame_type == H3_FRAME_GOAWAY) {
                    slot->h3_conn.goaway_sent = 1;
                }
                // Unknown frame types on control stream: silently ignored (RFC 9114 Section 7.2.8).
            }
        }
        // QPACK encoder/decoder streams (type 0x02, 0x03) are silently consumed.
        return 0;
    }

    // ── Client-initiated bidirectional stream (request stream) ──
    if (stream_type != 0x00) return 0; // skip server-initiated streams

    if (total_read == 0) {
        // Empty stream with FIN — reset with H3_NO_ERROR.
        taida_quiche.quiche_conn_stream_shutdown(slot->conn, stream_id, 1, 0);
        return 0;
    }

    // Decode H3 frames in the request stream.
    size_t pos = 0;
    int headers_seen = 0;
    H3Header request_headers[64];
    int request_header_count = 0;
    const unsigned char *request_body = NULL;
    size_t request_body_len = 0;
    // NB7-116: Concatenation buffer for multi-DATA frame bodies.
    // Bounded by stream_buf size (64KB). Multiple DATA frames are appended here
    // instead of overwriting request_body, matching Interpreter behavior.
    unsigned char body_buf[65536];
    size_t body_buf_len = 0;

    while (pos < total_read) {
        uint64_t frame_type, frame_length;
        size_t header_size;
        if (h3_decode_frame_header(stream_buf + pos, total_read - pos,
                                    &frame_type, &frame_length, &header_size) < 0) {
            // Malformed frame — reset stream with H3_ERR_FRAME_ERROR (0x0106).
            taida_quiche.quiche_conn_stream_shutdown(slot->conn, stream_id, 1, 0x0106);
            return 0;
        }

        const unsigned char *payload = stream_buf + pos + header_size;
        size_t payload_len = (size_t)frame_length;
        pos += header_size + payload_len;

        switch (frame_type) {
            case H3_FRAME_HEADERS: {
                if (headers_seen) {
                    // Duplicate HEADERS on same request stream — protocol error.
                    taida_quiche.quiche_conn_stream_shutdown(slot->conn, stream_id, 1, 0x0106);
                    return 0;
                }
                headers_seen = 1;

                // QPACK decode.
                request_header_count = h3_qpack_decode_block(
                    payload, payload_len, request_headers, 64);
                if (request_header_count < 0) {
                    // QPACK decode failure — 400 Bad Request.
                    unsigned char err_frame[256];
                    H3Header empty_hdrs[1];
                    int elen = h3_build_response_headers_frame(err_frame, sizeof(err_frame), 400, empty_hdrs, 0);
                    if (elen > 0) {
                        taida_quiche.quiche_conn_stream_send(slot->conn, stream_id,
                            err_frame, (size_t)elen, 0);
                    }
                    unsigned char data_frame[256];
                    const char *err_body = "Bad Request";
                    int dlen = h3_build_data_frame(data_frame, sizeof(data_frame),
                        (const unsigned char*)err_body, strlen(err_body));
                    if (dlen > 0) {
                        taida_quiche.quiche_conn_stream_send(slot->conn, stream_id,
                            data_frame, (size_t)dlen, 1 /*fin*/);
                    }
                    return 0;
                }
                break;
            }

            case H3_FRAME_DATA: {
                // NB7-116: DATA frame body — concatenate multi-DATA frames.
                // HTTP/3 allows request body to be split across multiple DATA frames.
                // Previously this overwrote request_body on each DATA frame, causing
                // body truncation to only the last frame. Now we append into a
                // dedicated buffer, matching Interpreter behavior (quic.rs:366-373).
                // bounded-copy discipline: body_buf lives on the stack, bounded by
                // stream_buf size (64KB).
                if (payload_len > 0) {
                    if (body_buf_len + payload_len <= sizeof(body_buf)) {
                        memcpy(body_buf + body_buf_len, payload, payload_len);
                        body_buf_len += payload_len;
                    }
                    // If body exceeds body_buf capacity, silently truncate
                    // (bounded-copy discipline — same as stream_buf overflow).
                }
                request_body = body_buf;
                request_body_len = body_buf_len;
                break;
            }

            case H3_FRAME_SETTINGS: {
                // NB7-84: SETTINGS MUST only be on control stream (RFC 9114 Section 7.2.4.1).
                taida_quiche.quiche_conn_stream_shutdown(slot->conn, stream_id, 1, 0x0105);
                return 0;
            }

            case H3_FRAME_GOAWAY: {
                // NB7-85: GOAWAY MUST only be on control stream (RFC 9114 Section 7.2.6).
                taida_quiche.quiche_conn_stream_shutdown(slot->conn, stream_id, 1, 0x0105);
                return 0;
            }

            default:
                // Unknown frame types: silently skip (RFC 9114 Section 7.2.8).
                break;
        }
    }

    if (!headers_seen) return 0; // No HEADERS frame — skip.

    // ── Extract request fields from QPACK-decoded headers ──
    H3RequestFields req_fields;
    h3_extract_request_fields(request_headers, request_header_count, &req_fields);
    if (!req_fields.ok) {
        // Invalid request (missing pseudo-headers, etc.) — 400 Bad Request.
        unsigned char err_frame[256];
        H3Header empty_hdrs[1];
        int elen = h3_build_response_headers_frame(err_frame, sizeof(err_frame), 400, empty_hdrs, 0);
        if (elen > 0) {
            taida_quiche.quiche_conn_stream_send(slot->conn, stream_id,
                err_frame, (size_t)elen, 0);
        }
        const char *err_body = "Bad Request";
        unsigned char data_frame[256];
        int dlen = h3_build_data_frame(data_frame, sizeof(data_frame),
            (const unsigned char*)err_body, strlen(err_body));
        if (dlen > 0) {
            taida_quiche.quiche_conn_stream_send(slot->conn, stream_id,
                data_frame, (size_t)dlen, 1);
        }
        if (req_fields.regular_headers) free(req_fields.regular_headers);
        return 0;
    }

    // ── Build request pack and dispatch handler ──
    char peer_host[64];
    inet_ntop(AF_INET, &slot->peer_addr.sin_addr, peer_host, sizeof(peer_host));
    int peer_port = ntohs(slot->peer_addr.sin_port);

    taida_val request_pack = h3_build_request_pack(
        &req_fields,
        request_body ? request_body : (const unsigned char*)"",
        request_body_len,
        peer_host, peer_port
    );
    free(req_fields.regular_headers);
    req_fields.regular_headers = NULL;

    // Dispatch to the Taida handler (same contract as h1/h2).
    H3ServeCtx ctx;
    ctx.handler = pool->handler;
    ctx.handler_arity = pool->handler_arity;
    ctx.request_count = &pool->request_count;
    ctx.max_requests = pool->max_requests;
    snprintf(ctx.peer_host, sizeof(ctx.peer_host), "%s", peer_host);
    ctx.peer_port = peer_port;

    taida_val response = h3_dispatch_request(&ctx, request_pack);

    // ── Extract response and encode H3 frames ──
    // Reuse H2ResponseFields — same handler response contract.
    H2ResponseFields resp;
    h2_extract_response_fields(response, &resp);

    int no_body = (resp.status >= 100 && resp.status < 200) ||
                  resp.status == 204 || resp.status == 205 || resp.status == 304;
    int has_body = resp.ok && resp.body && resp.body_len > 0 && !no_body;

    // Build response headers from handler output.
    H3Header resp_hdrs[32];
    int resp_hdr_count = 0;
    for (int i = 0; i < resp.header_count && resp_hdr_count < 32; i++) {
        snprintf(resp_hdrs[resp_hdr_count].name, sizeof(resp_hdrs[0].name),
                 "%s", resp.headers[i].name);
        snprintf(resp_hdrs[resp_hdr_count].value, sizeof(resp_hdrs[0].value),
                 "%s", resp.headers[i].value);
        resp_hdr_count++;
    }

    // Send HEADERS frame via quiche_conn_stream_send.
    unsigned char hdrs_frame[8192];
    int hlen = h3_build_response_headers_frame(hdrs_frame, sizeof(hdrs_frame),
                                                resp.status, resp_hdrs, resp_hdr_count);
    if (hlen > 0) {
        taida_quiche.quiche_conn_stream_send(slot->conn, stream_id,
            hdrs_frame, (size_t)hlen, !has_body ? 1 : 0 /*fin if no body*/);
    }

    // Send DATA frame if body exists.
    if (has_body) {
        unsigned char data_frame[65536];
        int dlen = h3_build_data_frame(data_frame, sizeof(data_frame),
                                        resp.body, resp.body_len);
        if (dlen > 0) {
            taida_quiche.quiche_conn_stream_send(slot->conn, stream_id,
                data_frame, (size_t)dlen, 1 /*fin=true*/);
        } else {
            // Body too large for buffer — send FIN without body.
            taida_quiche.quiche_conn_stream_send(slot->conn, stream_id,
                NULL, 0, 1 /*fin=true*/);
        }
    }

    h2_response_fields_free(&resp);
    taida_release(request_pack);
    taida_release(response);

    // Update last_peer_stream_id for GOAWAY tracking.
    slot->h3_conn.last_peer_stream_id = stream_id;

    return 1; // Successfully served a request.
}

// ── NET7-8b: serve_h3_loop — UDP socket + quiche_accept ──────────────────
//
// This is the entry point for the QUIC transport accept loop.
// It binds a UDP socket to 127.0.0.1:port and feeds incoming packets to
// quiche_accept(). Established connections are stored in the connection pool.
//
// Bounded-copy discipline: each recvfrom() packet is fed directly to
// quiche_accept/quiche_conn_recv without intermediate buffering.
// 1 packet = at most 1 materialization.
//
// Returns H3ServeResult with request count on success, or -1 on failure.
static H3ServeResult serve_h3_loop(int port, taida_val handler, int handler_arity,
                                    int64_t max_requests, int64_t timeout_ms,
                                    const char *cert_path, const char *key_path) {
    H3ServeResult fail_result = {-1};

    // Suppress SIGPIPE (same contract as h1/h2).
    signal(SIGPIPE, SIG_IGN);

    // Bind UDP socket to 127.0.0.1:port (same loopback contract as h1/h2).
    int udp_fd = socket(AF_INET, SOCK_DGRAM, 0);
    if (udp_fd < 0) {
        return fail_result;
    }

    int opt = 1;
    setsockopt(udp_fd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    inet_pton(AF_INET, "127.0.0.1", &addr.sin_addr);
    addr.sin_port = htons((unsigned short)port);

    if (bind(udp_fd, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        close(udp_fd);
        return fail_result;
    }

    // Set a receive timeout so we can periodically check shutdown/max_requests.
    {
        struct timeval tv;
        tv.tv_sec = 0;
        tv.tv_usec = 100000; // 100ms — matches h1/h2 accept timeout
        setsockopt(udp_fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    }

    // Create quiche_config.
    uint32_t version = taida_quiche.quiche_version();
    quiche_config *config = taida_quiche.quiche_config_new(version);
    if (!config) {
        close(udp_fd);
        return fail_result;
    }

    // NB7-3: cert/key loaded into quiche config (TLS 1.3 is mandatory for QUIC).
    if (taida_quiche.quiche_config_load_cert_chain_from_pem_file(config, cert_path) != 0) {
        taida_quiche.quiche_config_free(config);
        close(udp_fd);
        return fail_result;
    }
    if (taida_quiche.quiche_config_load_priv_key_from_pem_file(config, key_path) != 0) {
        taida_quiche.quiche_config_free(config);
        close(udp_fd);
        return fail_result;
    }

    // ALPN h3 only — matching design contract (no silent fallback).
    unsigned char alpn[] = QUICHE_H3_ALPN; // "\x02h3"
    if (taida_quiche.quiche_config_set_application_protos(config, alpn, sizeof(alpn) - 1) != 0) {
        taida_quiche.quiche_config_free(config);
        close(udp_fd);
        return fail_result;
    }

    // TLS verification and grease (matching quiche server defaults).
    taida_quiche.quiche_config_verify_peer(config, 0);
    taida_quiche.quiche_config_grease(config, 1);

    // Idle timeout — bounded to prevent connection leaks.
    uint64_t idle_timeout = (timeout_ms > 0) ? (uint64_t)timeout_ms : 30000; // default 30s
    taida_quiche.quiche_config_set_max_idle_timeout(config, idle_timeout);

    // NET7-12c: QUIC transport parameters — required for stream data flow.
    // Without these, quiche defaults to 0 (no data allowed on any stream).
    // Values match quiche server example defaults.
    taida_quiche.quiche_config_set_initial_max_data(config, 10 * 1024 * 1024);          // 10MB connection-level
    taida_quiche.quiche_config_set_initial_max_stream_data_bidi_local(config, 1024 * 1024);  // 1MB per local bidi stream
    taida_quiche.quiche_config_set_initial_max_stream_data_bidi_remote(config, 1024 * 1024); // 1MB per remote bidi stream
    taida_quiche.quiche_config_set_initial_max_stream_data_uni(config, 1024 * 1024);         // 1MB per uni stream
    taida_quiche.quiche_config_set_initial_max_streams_bidi(config, 128);                    // max 128 concurrent bidi streams
    taida_quiche.quiche_config_set_initial_max_streams_uni(config, 16);                      // max 16 concurrent uni streams

    // Initialize connection pool.
    int max_conn = (QUIC_MAX_CONNECTIONS < 256) ? QUIC_MAX_CONNECTIONS : 256;
    QuicConnPool pool;
    quic_pool_init(&pool, max_conn, handler, max_requests, timeout_ms, handler_arity,
                   cert_path, key_path);

    // ── NET7-8c: QUIC connection I/O event loop ─────────────────────────
    //
    // Unified accept + I/O processing loop. Each incoming datagram is
    // routed by DCID hash to either:
    //   - Known connection: quiche_conn_recv() → established check → send
    //   - Unknown DCID: quiche_accept() → quiche_conn_recv() → send → pool
    //
    // Bounded-copy discipline: 1 packet = at most 1 materialization.
    // No intermediate buffer between recvfrom() and quiche_conn_recv().

    unsigned char recv_buf[QUICHE_MAX_DATAGRAM_SIZE]; // 65527 (QUIC MTU budget)
    unsigned char send_buf[QUICHE_MAX_DATAGRAM_SIZE]; // bounded: matches recv_buf
    struct sockaddr_in peer_addr;
    socklen_t peer_len;
    struct sockaddr_in send_addr;
    socklen_t send_addr_len;
    H3ServeResult serve_result = {0};

    for (;;) {
        // Check shutdown and request limit before processing more.
        pthread_mutex_lock(&pool.mutex);
        int do_shutdown = pool.shutdown || quic_pool_requests_exhausted(&pool);
        pthread_mutex_unlock(&pool.mutex);

        if (do_shutdown) {
            break;
        }

        peer_len = sizeof(peer_addr);
        ssize_t rlen = recvfrom(udp_fd, recv_buf, sizeof(recv_buf), 0,
                               (struct sockaddr*)&peer_addr, &peer_len);

        if (rlen < 0) {
            if (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) {
                // Timeout — periodic maintenance pass.
                h3_conn_maintenance(&pool);
                continue;
            }
            // Fatal recvfrom error — break and clean up.
            break;
        }

        // bounded-copy: recv_buf is the only materialization of this packet.
        // No intermediate buffer.

        // Parse QUIC long header to extract DCID for connection routing.
        uint32_t pkt_version = 0;
        uint8_t pkt_type = 0;
        uint8_t pkt_dcid[20];
        size_t pkt_dcid_len = 0;
        uint8_t pkt_scid[20];
        size_t pkt_scid_len = 0;
        uint8_t pkt_token[20];
        size_t pkt_token_len = 0;

        int hdr_ok = taida_quiche.quiche_header_info(
            recv_buf, (size_t)rlen, 5,  // 5 byte DCID length hint for long header
            &pkt_version, &pkt_type,
            pkt_dcid, &pkt_dcid_len,
            pkt_scid, &pkt_scid_len,
            pkt_token, &pkt_token_len
        );

        if (hdr_ok != 0) {
            // Cannot parse header — skip packet (malformed or non-QUIC).
            continue;
        }

        if (pkt_dcid_len == 0) {
            // No DCID — malformed packet, skip.
            continue;
        }

        // Compute FNV-1a hash of the DCID for fast pool lookup.
        uint64_t dcid_hash = _fnv1a_64(pkt_dcid, pkt_dcid_len);

        // Look up existing connection by DCID hash.
        int slot_idx = quic_pool_find_by_dcid(&pool, dcid_hash);

        if (slot_idx >= 0) {
            // ── Known connection: feed to quiche_conn_recv() ────
            QuicConnSlot *slot = &pool.slots[slot_idx];

            if (!slot->conn || !slot->active) {
                // Slot metadata is inconsistent — skip this packet.
                continue;
            }

            // Check if connection is fully closed — free the slot.
            if (taida_quiche.quiche_conn_is_closed(slot->conn)) {
                quic_pool_close_slot(&pool, slot_idx);
                continue;
            }

            // If draining, close the slot and clean up.
            if (taida_quiche.quiche_conn_is_draining(slot->conn)) {
                quic_pool_close_slot(&pool, slot_idx);
                continue;
            }

            // Feed datagram to the QUIC connection.
            ssize_t recv_rc = taida_quiche.quiche_conn_recv(
                slot->conn,
                recv_buf, (size_t)rlen,
                (struct sockaddr*)&peer_addr, peer_len
            );

            if (recv_rc < 0 && recv_rc != -2) {
                // Fatal recv error — close the connection.
                // -2 = QUICHE_ERR_DONE (no more data to process)
                quic_pool_close_slot(&pool, slot_idx);
                continue;
            }

            // Connection established -> initialize H3 and process streams.
            if (taida_quiche.quiche_conn_is_established(slot->conn)) {
                slot->established = 1;

                // NET7-12c: Initialize H3 control stream on first established packet.
                if (!slot->h3_initialized) {
                    if (h3_init_control_stream(slot) < 0) {
                        quic_pool_close_slot(&pool, slot_idx);
                        continue;
                    }
                }

                // NET7-12c: Process all readable streams (H3 dispatch).
                void *readable = taida_quiche.quiche_conn_readable(slot->conn);
                if (readable) {
                    uint64_t stream_id;
                    while (taida_quiche.quiche_stream_iter_next(readable, &stream_id)) {
                        int result = h3_process_stream(slot, &pool, stream_id);
                        if (result == 1) {
                            // Valid request served — increment request count (NB7-66).
                            pthread_mutex_lock(&pool.mutex);
                            pool.request_count++;
                            int exhausted = quic_pool_requests_exhausted(&pool);
                            pthread_mutex_unlock(&pool.mutex);
                            if (exhausted) {
                                taida_quiche.quiche_stream_iter_free(readable);
                                goto shutdown_loop;
                            }
                        } else if (result == -1) {
                            // Connection-level error — close slot.
                            taida_quiche.quiche_stream_iter_free(readable);
                            quic_pool_close_slot(&pool, slot_idx);
                            goto next_packet;
                        }
                    }
                    taida_quiche.quiche_stream_iter_free(readable);
                }
            }

            // NET7-12c: Drain all pending outbound QUIC datagrams.
            if (quic_drain_send(udp_fd, slot->conn, send_buf, sizeof(send_buf)) < 0) {
                quic_pool_close_slot(&pool, slot_idx);
                continue;
            }
        } else {
            // ── Unknown DCID: new connection attempt ────
            quiche_conn *conn = taida_quiche.quiche_accept(
                pkt_dcid, pkt_dcid_len,           // DCID from packet header
                NULL, 0,                           // odcid (not needed for server)
                config,                            // TLS + protocol config
                (struct sockaddr*)&peer_addr,
                peer_len
            );

            if (!conn) {
                // Accept failed — invalid initial, version mismatch, etc.
                continue;
            }

            // First recv() to process the initial packet on the new connection.
            ssize_t recv_rc = taida_quiche.quiche_conn_recv(
                conn,
                recv_buf, (size_t)rlen,
                (struct sockaddr*)&peer_addr, peer_len
            );

            if (recv_rc < 0 && recv_rc != -2) {
                // Fatal recv error on new connection — free it.
                taida_quiche.quiche_conn_free(conn);
                continue;
            }

            // NET7-12c: Drain all handshake response datagrams.
            quic_drain_send(udp_fd, conn, send_buf, sizeof(send_buf));

            // Add to connection pool.
            int slot = quic_pool_find_or_create(&pool, conn, &peer_addr, dcid_hash);
            if (slot < 0) {
                // Pool full — close the connection immediately.
                taida_quiche.quiche_conn_free(conn);
                continue;
            }
        }

    next_packet:
        // Periodic maintenance (bounded-cost: scans 256 slots max).
        h3_conn_maintenance(&pool);
    }

shutdown_loop:
    // ── NET7-12d: Graceful shutdown: GOAWAY -> drain wait -> close ────────
    // Phase 7 contract: H3Connection shutdown is GOAWAY -> drain -> close.
    // The old code did `break -> quic_pool_destroy()` (immediate release).
    // Now we:
    //   1. Send GOAWAY on each active connection's control stream
    //   2. Call quiche_conn_close() for graceful QUIC-level close
    //   3. Drain all outbound datagrams (so GOAWAY reaches peers)
    //   4. Poll until all connections are closed or timeout (1 second)
    //   5. Destroy the pool
    serve_result.requests = pool.request_count;

    // Step 1 + 2 + 3: Send GOAWAY and close each active connection.
    {
        unsigned char goaway_buf[64];
        for (int i = 0; i < QUIC_MAX_CONNECTIONS; i++) {
            if (!pool.slots[i].active || !pool.slots[i].conn) continue;

            // Step 1: Send GOAWAY frame on the control stream (if initialized).
            if (pool.slots[i].ctrl_stream_created && !pool.slots[i].h3_conn.goaway_sent) {
                int goaway_len = h3_encode_goaway(goaway_buf, sizeof(goaway_buf),
                                                   pool.slots[i].h3_conn.last_peer_stream_id);
                if (goaway_len > 0) {
                    taida_quiche.quiche_conn_stream_send(
                        pool.slots[i].conn,
                        pool.slots[i].ctrl_stream_id,
                        goaway_buf, (size_t)goaway_len, false);
                    pool.slots[i].h3_conn.goaway_sent = 1;
                }
            }
            pool.slots[i].draining = 1;

            // Step 2: Initiate QUIC-level graceful close (H3_NO_ERROR = 0x0100).
            taida_quiche.quiche_conn_close(pool.slots[i].conn,
                                            1, 0x0100,
                                            (const uint8_t*)"shutdown", 8);

            // Step 3: Drain outbound datagrams so GOAWAY + CONNECTION_CLOSE reach peer.
            quic_drain_send(udp_fd, pool.slots[i].conn, send_buf, sizeof(send_buf));
        }
    }

    // Step 4: Poll for all connections to close (bounded drain wait, 1 second max).
    // NB7-67: This replaces the immediate quic_pool_destroy().
    {
        struct timespec drain_start;
        clock_gettime(CLOCK_MONOTONIC, &drain_start);
        const int64_t drain_timeout_ms = 1000; // 1 second max drain wait

        for (;;) {
            // Check if all connections are closed or draining.
            int all_done = 1;
            for (int i = 0; i < QUIC_MAX_CONNECTIONS; i++) {
                if (!pool.slots[i].active || !pool.slots[i].conn) continue;
                if (!taida_quiche.quiche_conn_is_closed(pool.slots[i].conn) &&
                    !taida_quiche.quiche_conn_is_draining(pool.slots[i].conn)) {
                    all_done = 0;
                    break;
                }
            }
            if (all_done) break;

            // Check drain timeout.
            struct timespec now;
            clock_gettime(CLOCK_MONOTONIC, &now);
            int64_t elapsed_ms = (now.tv_sec - drain_start.tv_sec) * 1000
                               + (now.tv_nsec - drain_start.tv_nsec) / 1000000;
            if (elapsed_ms >= drain_timeout_ms) break;

            // Process any incoming packets during drain (peers may send ACKs).
            peer_len = sizeof(peer_addr);
            ssize_t drain_rlen = recvfrom(udp_fd, recv_buf, sizeof(recv_buf), 0,
                                          (struct sockaddr*)&peer_addr, &peer_len);
            if (drain_rlen > 0) {
                // Route to the right connection using existing header parsing.
                uint8_t dr_dcid[20];
                size_t dr_dcid_len = 0;
                uint32_t dr_ver = 0;
                uint8_t dr_type = 0;
                uint8_t dr_scid[20];
                size_t dr_scid_len = 0;
                uint8_t dr_token[20];
                size_t dr_token_len = 0;

                int hdr_rc = taida_quiche.quiche_header_info(
                    recv_buf, (size_t)drain_rlen, 5,
                    &dr_ver, &dr_type,
                    dr_dcid, &dr_dcid_len,
                    dr_scid, &dr_scid_len,
                    dr_token, &dr_token_len);
                if (hdr_rc >= 0) {
                    uint64_t dcid_hash = _fnv1a_64(dr_dcid, dr_dcid_len);
                    int slot_idx = quic_pool_find_by_dcid(&pool, dcid_hash);
                    if (slot_idx >= 0 && pool.slots[slot_idx].conn) {
                        taida_quiche.quiche_conn_recv(
                            pool.slots[slot_idx].conn,
                            recv_buf, (size_t)drain_rlen,
                            (struct sockaddr*)&peer_addr, peer_len);
                        // Fire timer if available.
                        if (taida_quiche.quiche_conn_on_timeout) {
                            taida_quiche.quiche_conn_on_timeout(pool.slots[slot_idx].conn);
                        }
                        // Drain any response datagrams (ACKs, CONNECTION_CLOSE retransmit).
                        quic_drain_send(udp_fd, pool.slots[slot_idx].conn,
                                        send_buf, sizeof(send_buf));
                    }
                }
            } else {
                // No packet received — fire timer on all draining connections.
                if (taida_quiche.quiche_conn_on_timeout) {
                    for (int i = 0; i < QUIC_MAX_CONNECTIONS; i++) {
                        if (!pool.slots[i].active || !pool.slots[i].conn) continue;
                        if (pool.slots[i].draining) {
                            taida_quiche.quiche_conn_on_timeout(pool.slots[i].conn);
                            quic_drain_send(udp_fd, pool.slots[i].conn,
                                            send_buf, sizeof(send_buf));
                        }
                    }
                }
            }
        }
    }

    // Step 5: Destroy pool (all connections freed).
    quic_pool_destroy(&pool);

    taida_quiche.quiche_config_free(config);
    close(udp_fd);

    return serve_result;
}

// ── taida_net_h3_serve ────────────────────────────────────────────────────
// NET7-2a/2b/2c: HTTP/3 server reference implementation.
//
// Phase 2 reference: This function establishes the HTTP/3 handler contract,
// QPACK encode/decode semantics, stream lifecycle, and graceful shutdown
// for the Native backend.
//
// QUIC transport: Phase 2 uses a quiche-based approach via dlopen.
// If quiche (libquiche.so) is not available at runtime, returns
// H3QuicUnavailable — analogous to how TLS returns TlsError when
// OpenSSL is missing.
//
// Design contracts (NET_DESIGN.md):
//   - UDP bind to 127.0.0.1:port (same loopback contract as h1/h2)
//   - TLS 1.3 mandatory (QUIC includes TLS in handshake)
//   - cert/key required (validated before reaching here)
//   - 0-RTT: default-off, not exposed
//   - Handler dispatch: same 14-field request pack as h1/h2
//   - Graceful shutdown: GOAWAY → drain → close
//   - Bounded-copy discipline: 1 packet = at most 1 materialization
//   - No aggregate buffer above packet boundary
// (H3ServeResult typedef moved before NET7-8b pool struct, see above)

static H3ServeResult taida_net_h3_serve(int port, taida_val handler, int handler_arity,
                                         int64_t max_requests, int64_t timeout_ms,
                                         const char *cert_path, const char *key_path) {
    H3ServeResult fail_result = {-1};

    // NB7-9/NB7-10: Run embedded self-tests to validate QPACK round-trip
    // and H3 request pseudo-header validation at every H3 serve invocation.
    // This ensures Phase 2 reference semantics are correct before entering
    // the QUIC transport layer.
    {
        int selftest_rc = h3_run_selftests();
        if (selftest_rc != 0) {
            fail_result.requests = -3; // -3 = selftest failure
            return fail_result;
        }
    }

    // NET7-8a: QUIC transport requires a QUIC library (quiche).
    // Use the taida_quiche FFI contract (dlopen + dlsym) instead of
    // raw dlopen — this mirrors the taida_ossl pattern for TLS.
    //
    // If quiche (libquiche.so) is not available at runtime, returns
    // H3QuicUnavailable (-1) — analogous to how TLS returns TlsError
    // when OpenSSL is missing.
    //
    // The H3 protocol layer (QPACK, frames, stream state, request/response
    // mapping, graceful shutdown) is fully implemented above. The QUIC
    // transport binding is gated on quiche availability.
    //
    // If taida_quiche_load() succeeds, the full H3 serve loop would:
    //   1. Create quiche_config with cert/key and TLS 1.3
    //   2. Bind UDP socket to 127.0.0.1:port
    //   3. Accept QUIC connections (quiche_accept / quiche_conn_new_with_tls)
    //   4. For each QUIC connection:
    //      a. Complete handshake
    //      b. Open control streams (send SETTINGS)
    //      c. Accept request streams
    //      d. Read H3 frames (HEADERS + DATA) from request streams
    //      e. Decode QPACK headers → extract request fields
    //      f. Build request pack → dispatch handler → extract response
    //      g. Encode QPACK response headers → send HEADERS + DATA frames
    //      h. Track request count against max_requests
    //   5. On shutdown: send GOAWAY, drain in-flight streams, close connections

    if (!taida_quiche_load()) {
        // QUIC transport library not available.
        // All H3 protocol semantics (QPACK, frames, streams, request mapping,
        // graceful shutdown) are implemented and tested. Only the QUIC
        // transport binding requires the external library.
        return fail_result;
    }

    // NET7-8b/8c/12c: Wire the QUIC transport I/O event loop.
    // 8b: UDP socket accept loop + quiche_accept (DONE)
    // 8c: QUIC connection I/O event loop (recv/send/established) (DONE)
    // 12c: QUIC stream dispatch -> H3 decode -> handler -> response encode (DONE)
    H3ServeResult loop_result = serve_h3_loop(port, handler, handler_arity,
                                               max_requests, timeout_ms,
                                               cert_path, key_path);
    return loop_result;
}

// ── httpServe(port, handler, maxRequests, timeoutMs, maxConnections) ──
// HTTP/1.1 server v2+v3: keep-alive, chunked TE, pthread pool, maxConnections.
// NET3-5a: handler_arity added — 2 = streaming writer, 1 = one-shot, -1 = unknown.
// v5: tls parameter added. 0 = plaintext (v4 compat), non-zero = BuchiPack @(cert, key) = HTTPS via OpenSSL dlopen.
// Returns Async[Result[@(ok: Bool, requests: Int), _]]
taida_val taida_net_http_serve(taida_val port, taida_val handler, taida_val max_requests, taida_val timeout_ms, taida_val max_connections, taida_val tls, taida_val handler_type_tag, taida_val handler_arity) {
    // NB3-5: Suppress SIGPIPE process-wide. Without this, writev() or
    // send() on a peer-closed socket delivers SIGPIPE which terminates the
    // process before the return-value error path can execute. This is the
    // standard pattern for HTTP servers (nginx, Apache, Go net/http all do
    // the same). MSG_NOSIGNAL covers send() individually, but writev() has
    // no per-call flag — signal(SIGPIPE, SIG_IGN) is the only portable way.
    signal(SIGPIPE, SIG_IGN);

    // NB-2: port range validation (parity with Interpreter/JS)
    if (port < 0 || port > 65535) {
        char errbuf[256];
        snprintf(errbuf, sizeof(errbuf), "httpServe: port must be 0-65535, got %lld", (long long)port);
        return taida_async_resolved(taida_net_result_fail("PortError", errbuf));
    }

    // NB-31: handler callable check using compile-time type tag.
    {
        int callable = 0;
        if (handler_type_tag == 6 || handler_type_tag == 10) {
            callable = 1;
        } else if (handler_type_tag == -1) {
            callable = TAIDA_IS_CALLABLE(handler);
        }
        if (!callable) {
            return taida_async_resolved(taida_net_result_fail("TypeError", "httpServe: handler must be a Function"));
        }
    }

    // NET5-4a: TLS configuration — replaced Phase 2 stub with actual implementation.
    // tls is a BuchiPack pointer (non-zero = object) or 0 (default = plaintext).
    // NB5-16: Non-zero non-BuchiPack tls must NOT silently fall back to plaintext.
    // Only 0 (default) and valid BuchiPack pointers are accepted.
    // v6 NET6-1b: protocol field support for h2 opt-in.
    OSSL_SSL_CTX *ssl_ctx = NULL;
    const char *requested_protocol = NULL;
    // NET6-3a: hoisted cert/key paths so h2 branch can call taida_net_h2_serve directly.
    const char *h2_cert_path = NULL;
    const char *h2_key_path = NULL;
    if (tls != 0 && !TAIDA_IS_PACK(tls)) {
        // Non-BuchiPack non-zero value (e.g. tls=42) → reject.
        fprintf(stderr, "RuntimeError: httpServe: tls must be a BuchiPack @(cert: Str, key: Str) or @(), got non-pack value\n");
        fflush(stderr);
        exit(1);
    }
    if (tls != 0) {
        taida_val *pack = (taida_val *)tls;
        int64_t field_count = pack[1];

        // v6 NET6-1b: Extract protocol field if present.
        // NB6-10: Use taida_pack_has_hash() to confirm field existence first,
        // then resolve UNKNOWN tags via taida_runtime_detect_tag().
        // This correctly handles dynamic packs where the compiler couldn't
        // determine the field tag statically (e.g., `@(protocol <= x)` with
        // x being a non-Str value passed through a function parameter).
        taida_val proto_hash = taida_str_hash((taida_val)"protocol");
        if (taida_pack_has_hash(tls, proto_hash)) {
            // protocol field exists in the pack — now check its type
            taida_val proto_tag = taida_pack_get_field_tag(tls, proto_hash);
            if (proto_tag == TAIDA_TAG_UNKNOWN) {
                // Dynamic case: tag not set at compile time, resolve at runtime
                taida_val proto_val = taida_pack_get(tls, proto_hash);
                proto_tag = taida_runtime_detect_tag(proto_val);
            }
            if (proto_tag == TAIDA_TAG_STR) {
                taida_val proto_val = taida_pack_get(tls, proto_hash);
                if (proto_val && proto_val > 4096) {
                    requested_protocol = (const char *)proto_val;
                }
            } else if (proto_tag == TAIDA_TAG_INT) {
                taida_val proto_val = taida_pack_get(tls, proto_hash);
                int64_t ordinal = (int64_t)proto_val;
                // Sync with `crate::net_surface::http_protocol_ordinal_to_wire`.
                if (ordinal == 0) {
                    requested_protocol = "h1.1";
                } else if (ordinal == 1) {
                    requested_protocol = "h2";
                } else if (ordinal == 2) {
                    requested_protocol = "h3";
                } else {
                    char proto_err[256];
                    snprintf(proto_err, sizeof(proto_err),
                        "httpServe: unknown HttpProtocol ordinal %" PRId64 ". Expected 0 (H1), 1 (H2), or 2 (H3).",
                        ordinal);
                    return taida_async_resolved(taida_net_result_fail("ProtocolError", proto_err));
                }
            } else {
                // protocol field exists but is not Str / HttpProtocol ordinal → ProtocolError
                char proto_err[256];
                taida_val proto_val = taida_pack_get(tls, proto_hash);
                char val_buf[64];
                taida_format_value(proto_tag, proto_val, val_buf, sizeof(val_buf));
                snprintf(proto_err, sizeof(proto_err),
                    "httpServe: protocol must be HttpProtocol or Str, got %s",
                    val_buf);
                return taida_async_resolved(taida_net_result_fail("ProtocolError", proto_err));
            }
        }

        // NET7-2a: Check h3 protocol BEFORE cert/key file load.
        // h3 uses QUIC/TLS1.3, NOT the OpenSSL TCP-TLS path.
        // cert/key are validated here but not loaded through OpenSSL —
        // the QUIC library handles TLS 1.3 internally.
        if (requested_protocol != NULL && strcmp(requested_protocol, "h3") == 0) {
            taida_val cert_val = taida_pack_get(tls, taida_str_hash((taida_val)"cert"));
            taida_val key_val = taida_pack_get(tls, taida_str_hash((taida_val)"key"));
            int has_cert = (cert_val && cert_val > 4096);
            int has_key = (key_val && key_val > 4096);
            if (!has_cert || !has_key) {
                return taida_async_resolved(taida_net_result_fail("ProtocolError",
                    "httpServe: HTTP/3 (protocol: \"h3\") requires TLS (cert + key)."));
            }
            // NET7-2a: Dispatch to H3 serve path.
            // cert/key paths are passed to the QUIC library (not to OpenSSL).
            const char *h3_cert = (const char *)cert_val;
            const char *h3_key = (const char *)key_val;
            H3ServeResult h3_result = taida_net_h3_serve(
                (int)port, handler, (int)handler_arity,
                max_requests, timeout_ms,
                h3_cert, h3_key);
            if (h3_result.requests == -1) {
                // QUIC transport library (libquiche.so) not available
                return taida_async_resolved(taida_net_result_fail("H3QuicUnavailable",
                    "httpServe: HTTP/3 requires QUIC transport (libquiche.so). "
                    "Install quiche or equivalent QUIC library. "
                    "The HTTP/3 protocol layer (QPACK, frames, stream management) "
                    "is ready; only the QUIC transport binding is missing."));
            }
            if (h3_result.requests == -2) {
                // quiche found but integration pending
                return taida_async_resolved(taida_net_result_fail("H3TransportPending",
                    "httpServe: HTTP/3 QUIC transport library found but integration "
                    "is pending. The HTTP/3 protocol layer (QPACK, frame encoding, "
                    "stream state, request/response mapping, graceful shutdown) is "
                    "implemented. QUIC transport wiring will complete in Phase 2 hardening."));
            }
            if (h3_result.requests == -3) {
                // NB7-9/NB7-10: H3 protocol layer self-test failed
                return taida_async_resolved(taida_net_result_fail("H3SelftestFailed",
                    "httpServe: HTTP/3 protocol layer self-test failed. "
                    "QPACK encode/decode round-trip or request pseudo-header "
                    "validation is broken."));
            }
            // Success
            taida_val h3_inner = taida_pack_new(2);
            taida_pack_set_hash(h3_inner, 0, taida_str_hash((taida_val)"ok"));
            taida_pack_set(h3_inner, 0, 1);
            taida_pack_set_tag(h3_inner, 0, TAIDA_TAG_BOOL);
            taida_pack_set_hash(h3_inner, 1, taida_str_hash((taida_val)"requests"));
            taida_pack_set(h3_inner, 1, (taida_val)h3_result.requests);
            taida_pack_set_tag(h3_inner, 1, TAIDA_TAG_INT);
            return taida_async_resolved(taida_net_result_ok(h3_inner));
        }

        if (field_count > 0) {
            // Check if we have cert/key fields (not just protocol).
            taida_val cert_val = taida_pack_get(tls, taida_str_hash((taida_val)"cert"));
            taida_val key_val = taida_pack_get(tls, taida_str_hash((taida_val)"key"));

            if ((cert_val && cert_val > 4096) || (key_val && key_val > 4096)) {
                // Non-empty tls pack with cert/key → extract cert and key paths, initialize TLS.
                // Load OpenSSL via dlopen.
                if (!taida_ossl_load()) {
                    return taida_async_resolved(taida_net_result_fail("TlsError",
                        "httpServe: TLS/HTTPS requires OpenSSL (libssl.so). "
                        "Install libssl3 or equivalent."));
                }
                if (!cert_val || cert_val <= 4096) {
                    return taida_async_resolved(taida_net_result_fail("TlsError",
                        "httpServe: tls config requires 'cert' field (path to PEM certificate file)"));
                }
                if (!key_val || key_val <= 4096) {
                    return taida_async_resolved(taida_net_result_fail("TlsError",
                        "httpServe: tls config requires 'key' field (path to PEM private key file)"));
                }
                h2_cert_path = (const char *)cert_val;
                h2_key_path = (const char *)key_val;

                // Create SSL_CTX with cert/key.
                char tls_errbuf[512];
                ssl_ctx = taida_tls_create_ctx(h2_cert_path, h2_key_path, tls_errbuf, sizeof(tls_errbuf));
                if (!ssl_ctx) {
                    return taida_async_resolved(taida_net_result_fail("TlsError", tls_errbuf));
                }
            }
            // else: pack has fields but no cert/key (e.g. only protocol) → fall through to protocol check
        }
        // else: empty @() pack → plaintext, fall through
    }

    // v6 NET6-1b / NET6-3a / v7 NET7-1c: Protocol validation and dispatch.
    // HTTP/2 is opt-in. Explicit h1.1 falls through to h1 path.
    // h2 without TLS cert/key → ProtocolError (h2c out of scope per design).
    // h2 with TLS cert/key → taida_net_h2_serve (NET6-3a unlocked).
    // h3 is fully handled BEFORE cert/key loading (NB7-6 fix above).
    // Unknown protocol values are rejected immediately.
    if (requested_protocol != NULL) {
        if (strcmp(requested_protocol, "h1.1") == 0 || strcmp(requested_protocol, "http/1.1") == 0) {
            // Explicit HTTP/1.1 — same as default, fall through to h1 path.
        } else if (strcmp(requested_protocol, "h2") == 0) {
            // NET6-3a: HTTP/2 path unlocked.
            // h2c (cleartext HTTP/2) is out of scope — TLS is required.
            if (!h2_cert_path || !h2_key_path) {
                if (ssl_ctx) { taida_ossl.SSL_CTX_free(ssl_ctx); }
                return taida_async_resolved(taida_net_result_fail("ProtocolError",
                    "httpServe: HTTP/2 (protocol: \"h2\") requires TLS. "
                    "Provide tls: @(cert: \"...\", key: \"...\", protocol: \"h2\")."));
            }
            // ssl_ctx was created with taida_tls_create_ctx (h1 ctx) in the TLS block above.
            // taida_net_h2_serve creates its own h2-specific ssl_ctx via taida_tls_create_ctx_h2.
            // Free the h1 ssl_ctx before delegating to h2 serve.
            if (ssl_ctx) { taida_ossl.SSL_CTX_free(ssl_ctx); ssl_ctx = NULL; }
            H2ServeResult h2_result = taida_net_h2_serve(
                (int)port, handler, (int)handler_arity,
                max_requests, timeout_ms,
                h2_cert_path, h2_key_path);
            if (h2_result.requests < 0) {
                return taida_async_resolved(taida_net_result_fail("H2ServeError",
                    "httpServe: HTTP/2 server failed to start. "
                    "Check cert/key paths and OpenSSL availability."));
            }
            taida_val h2_inner = taida_pack_new(2);
            taida_pack_set_hash(h2_inner, 0, taida_str_hash((taida_val)"ok"));
            taida_pack_set(h2_inner, 0, 1);
            taida_pack_set_tag(h2_inner, 0, TAIDA_TAG_BOOL);
            taida_pack_set_hash(h2_inner, 1, taida_str_hash((taida_val)"requests"));
            taida_pack_set(h2_inner, 1, (taida_val)h2_result.requests);
            taida_pack_set_tag(h2_inner, 1, TAIDA_TAG_INT);
            return taida_async_resolved(taida_net_result_ok(h2_inner));
        } else {
            // Unknown protocol. h3 is already handled before cert/key loading (NB7-6).
            if (ssl_ctx) { taida_ossl.SSL_CTX_free(ssl_ctx); }
            char proto_err[256];
            snprintf(proto_err, sizeof(proto_err),
                "httpServe: unknown protocol \"%s\". Supported values: \"h1.1\", \"h2\", \"h3\"",
                requested_protocol);
            return taida_async_resolved(taida_net_result_fail("ProtocolError", proto_err));
        }
    }

    // NET2-5d: maxConnections (default 128, <= 0 falls back to 128)
    int64_t max_conn = (max_connections > 0) ? max_connections : 128;

    // Bind to 127.0.0.1:port (v1 contract: always loopback)
    int sockfd = socket(AF_INET, SOCK_STREAM, 0);
    if (sockfd < 0) {
        char errbuf[256];
        snprintf(errbuf, sizeof(errbuf), "httpServe: failed to bind to 127.0.0.1:%d: %s", (int)port, strerror(errno));
        return taida_async_resolved(taida_net_result_fail("BindError", errbuf));
    }

    int opt = 1;
    setsockopt(sockfd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    inet_pton(AF_INET, "127.0.0.1", &addr.sin_addr);
    addr.sin_port = htons((unsigned short)port);

    if (bind(sockfd, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        char errbuf[256];
        snprintf(errbuf, sizeof(errbuf), "httpServe: failed to bind to 127.0.0.1:%d: %s", (int)port, strerror(errno));
        close(sockfd);
        return taida_async_resolved(taida_net_result_fail("BindError", errbuf));
    }

    if (listen(sockfd, 128) < 0) {
        char errbuf[256];
        snprintf(errbuf, sizeof(errbuf), "httpServe: listen failed: %s", strerror(errno));
        close(sockfd);
        return taida_async_resolved(taida_net_result_fail("BindError", errbuf));
    }

    // C27B-014: opt-in port announcement for soak proxy / runbook.
    // Default OFF (env unset). When TAIDA_NET_ANNOUNCE_PORT=1, resolve
    // the actual bound port via getsockname (handles port=0) and emit
    // a single stdout line. 3-backend parity with interpreter / JS.
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

    // NET2-5c: Create thread pool
    // Number of worker threads = min(maxConnections, 16) to avoid thread explosion.
    // Each worker handles one connection at a time with keep-alive loop.
    int num_workers = (int)max_conn;
    if (num_workers > 16) num_workers = 16;
    if (num_workers < 1) num_workers = 1;

    NetThreadPool pool;
    net_pool_init(&pool, (int)max_conn + 16, handler, max_requests, timeout_ms, (int)handler_arity);
    pool.ssl_ctx = ssl_ctx; // NET5-4a: NULL = plaintext, non-NULL = TLS

    pthread_t *workers = (pthread_t*)TAIDA_MALLOC(sizeof(pthread_t) * (size_t)num_workers, "net_workers");
    for (int i = 0; i < num_workers; i++) {
        pthread_create(&workers[i], NULL, net_worker_thread, &pool);
    }

    // Accept loop: accept connections and enqueue to worker pool
    for (;;) {
        // NB2-14: Single critical section for both request-limit check and maxConnections wait.
        // Eliminates TOCTOU window from the original unlock-relock pattern.
        pthread_mutex_lock(&pool.mutex);
        if (net_pool_requests_exhausted(&pool)) {
            pthread_mutex_unlock(&pool.mutex);
            break;
        }
        while (pool.active_connections + pool.queue_count >= (int)max_conn && !net_pool_requests_exhausted(&pool)) {
            pthread_cond_wait(&pool.cond_done, &pool.mutex);
        }
        if (net_pool_requests_exhausted(&pool)) {
            pthread_mutex_unlock(&pool.mutex);
            break;
        }
        pthread_mutex_unlock(&pool.mutex);

        // Set a short accept timeout so we can re-check request limits
        {
            struct timeval tv;
            tv.tv_sec = 0;
            tv.tv_usec = 100000;  // 100ms
            setsockopt(sockfd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
        }

        struct sockaddr_in peer_addr;
        socklen_t peer_len = sizeof(peer_addr);
        int client_fd = accept(sockfd, (struct sockaddr*)&peer_addr, &peer_len);
        if (client_fd < 0) {
            if (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) {
                continue; // timeout or interrupt, re-check limits
            }
            // Fatal accept error
            break;
        }

        // Enqueue to worker pool
        pthread_mutex_lock(&pool.mutex);
        // NB2-10: Close fd if queue is full to prevent fd leak
        if (net_pool_enqueue(&pool, client_fd, peer_addr) < 0) {
            pthread_mutex_unlock(&pool.mutex);
            close(client_fd);
        } else {
            pthread_cond_signal(&pool.cond_available);
            pthread_mutex_unlock(&pool.mutex);
        }
    }

    // NB2-6: Shutdown — close server socket early, drain queued fds, signal workers.
    // Close the listening socket first so no new connections can arrive.
    close(sockfd);

    // Signal all workers to exit and drain any queued-but-unprocessed client fds.
    pthread_mutex_lock(&pool.mutex);
    pool.shutdown = 1;
    // Drain unprocessed queue entries to prevent fd leak
    {
        NetClientSlot drain_slot;
        while (net_pool_dequeue(&pool, &drain_slot) == 0) {
            close(drain_slot.client_fd);
        }
    }
    pthread_cond_broadcast(&pool.cond_available);
    pthread_mutex_unlock(&pool.mutex);

    // Workers currently in recv() will time out within SO_RCVTIMEO (effective_timeout ms).
    for (int i = 0; i < num_workers; i++) {
        pthread_join(workers[i], NULL);
    }

    int64_t final_count = pool.request_count;

    free(workers);
    net_pool_destroy(&pool);

    // NET5-4a: Free TLS context.
    if (ssl_ctx && taida_ossl.loaded) {
        taida_ossl.SSL_CTX_free(ssl_ctx);
    }

    // Server completed successfully
    taida_val ok_inner = taida_pack_new(2);
    taida_pack_set_hash(ok_inner, 0, taida_str_hash((taida_val)"ok"));
    taida_pack_set(ok_inner, 0, 1);  // true
    taida_pack_set_tag(ok_inner, 0, TAIDA_TAG_BOOL);
    taida_pack_set_hash(ok_inner, 1, taida_str_hash((taida_val)"requests"));
    taida_pack_set(ok_inner, 1, (taida_val)final_count);

    return taida_async_resolved(taida_net_result_ok(ok_inner));
}

/* ============================================================================ */
/* RC2.5: Addon dispatch (dlopen + v1 ABI)                                      */
/*                                                                              */
/* Single entry point from Cranelift IR:                                        */
/*   int64_t taida_addon_call(                                                  */
/*       const char* package_id,                                                */
/*       const char* cdylib_path,                                               */
/*       const char* function_name,                                             */
/*       int64_t argc,                                                          */
/*       int64_t argv_pack);  // Taida Pack built by lowering                   */
/*                                                                              */
/* Frozen design (.dev/RC2_5_DESIGN.md §A):                                     */
/*   - Lazy dlopen on first call for a given package_id                         */
/*   - Per-process registry protected by pthread_mutex                          */
/*   - ABI v1 struct layout byte-compatible with crates/addon-rs/src/abi.rs     */
/*   - init callback invoked exactly once after successful handshake            */
/*   - dlopen / dlsym / ABI mismatch / init failure are hard fail               */
/*     (fputs to stderr + exit(1)). The addon is language foundation; if it     */
/*     can't even load there is no recovery path the user could take.           */
/*   - Status::Error from a successful call is converted to a catchable Taida   */
/*     `AddonError` variant via taida_throw — RC2.5-3a Phase 3.                 */
/*                                                                              */
/* Phase 1 scope:                                                               */
/*   - Dispatcher present and linkable                                          */
/*   - Minimal value bridge: Int / Str / Bool / Unit / Pack (pack as argv only) */
/*                                                                              */
/* Phase 3 scope (RC2.5-3a/3b/3c):                                              */
/*   - Status::Error → catchable AddonError variant (taida_throw)               */
/*   - dlopen/dlsym/ABI/init failure → hard fail (taida_addon_fail), with the   */
/*     spec-compliant "taida: addon load failed: <pkg>: <detail>" format.       */
/*   - Windows abstraction macros (LoadLibraryA / GetProcAddress / FreeLibrary) */
/*     so the addon block can compile on Windows. v1 scope: smoke test only;    */
/*     real Windows execution coverage is RC3+ (RC2.5B-005).                    */
/* ============================================================================ */

/* ---------------- ABI v1 type definitions (byte-compatible with Rust) ---------------- */

/* TaidaAddonStatus (repr u32) */
typedef enum {
    TAIDA_ADDON_STATUS_OK = 0,
    TAIDA_ADDON_STATUS_ERROR = 1,
    TAIDA_ADDON_STATUS_ABI_MISMATCH = 2,
    TAIDA_ADDON_STATUS_INVALID_STATE = 3,
    TAIDA_ADDON_STATUS_UNSUPPORTED_VALUE = 4,
    TAIDA_ADDON_STATUS_NULL_POINTER = 5,
    TAIDA_ADDON_STATUS_ARITY_MISMATCH = 6,
} TaidaAddonStatusV1;

/* TaidaAddonValueTag (repr u32) — DIFFERENT numbering from the native runtime
 * internal TAIDA_TAG_* constants. The C dispatcher must translate between
 * native tags (TAIDA_TAG_INT=0, TAIDA_TAG_STR=3, etc.) and addon tags below. */
#define TAIDA_ADDON_TAG_UNIT  0
#define TAIDA_ADDON_TAG_INT   1
#define TAIDA_ADDON_TAG_FLOAT 2
#define TAIDA_ADDON_TAG_BOOL  3
#define TAIDA_ADDON_TAG_STR   4
#define TAIDA_ADDON_TAG_BYTES 5
#define TAIDA_ADDON_TAG_LIST  6
#define TAIDA_ADDON_TAG_PACK  7

/* Forward declarations */
struct TaidaAddonValueV1;
struct TaidaAddonErrorV1;
struct TaidaHostV1;

/* TaidaAddonValueV1 (repr C, 16 bytes on LP64) */
typedef struct TaidaAddonValueV1 {
    uint32_t tag;
    uint32_t _reserved;
    void    *payload;
} TaidaAddonValueV1;

/* TaidaAddonErrorV1 (repr C, 16 bytes on LP64) */
typedef struct TaidaAddonErrorV1 {
    uint32_t    code;
    uint32_t    _reserved;
    const char *message;
} TaidaAddonErrorV1;

/* TaidaAddonIntPayload */
typedef struct {
    int64_t value;
} TaidaAddonIntPayloadV1;

/* TaidaAddonFloatPayload */
typedef struct {
    double value;
} TaidaAddonFloatPayloadV1;

/* TaidaAddonBoolPayload */
typedef struct {
    uint8_t value;
} TaidaAddonBoolPayloadV1;

/* TaidaAddonBytesPayload (also used for Str) */
typedef struct {
    const uint8_t *ptr;
    size_t         len;
} TaidaAddonBytesPayloadV1;

/* TaidaAddonListPayload */
typedef struct {
    TaidaAddonValueV1 **items;
    size_t              len;
} TaidaAddonListPayloadV1;

/* TaidaAddonPackEntryV1 */
typedef struct {
    const char        *name;
    TaidaAddonValueV1 *value;
} TaidaAddonPackEntryV1;

/* TaidaAddonPackPayload */
typedef struct {
    TaidaAddonPackEntryV1 *entries;
    size_t                 len;
} TaidaAddonPackPayloadV1;

/* TaidaAddonFunctionV1 (repr C, 24 bytes on LP64) */
typedef TaidaAddonStatusV1 (*TaidaAddonCallFn)(
    const TaidaAddonValueV1 *args_ptr,
    uint32_t                 args_len,
    TaidaAddonValueV1      **out_value,
    TaidaAddonErrorV1      **out_error);

typedef struct TaidaAddonFunctionV1 {
    const char       *name;
    uint32_t          arity;
    /* natural 4-byte pad on LP64 before the fn ptr */
    uint32_t          _pad;
    TaidaAddonCallFn  call;
} TaidaAddonFunctionV1;

/* TaidaHostV1 — forward declare the callbacks first */
typedef TaidaAddonValueV1 *(*TaidaHostValueNewUnit)(const struct TaidaHostV1 *host);
typedef TaidaAddonValueV1 *(*TaidaHostValueNewInt) (const struct TaidaHostV1 *host, int64_t v);
typedef TaidaAddonValueV1 *(*TaidaHostValueNewFlt) (const struct TaidaHostV1 *host, double v);
typedef TaidaAddonValueV1 *(*TaidaHostValueNewBool)(const struct TaidaHostV1 *host, uint8_t v);
typedef TaidaAddonValueV1 *(*TaidaHostValueNewBytes)(
    const struct TaidaHostV1 *host, const uint8_t *bytes, size_t len);
typedef TaidaAddonValueV1 *(*TaidaHostValueNewList)(
    const struct TaidaHostV1 *host, TaidaAddonValueV1 *const *items, size_t len);
typedef TaidaAddonValueV1 *(*TaidaHostValueNewPack)(
    const struct TaidaHostV1 *host,
    const char *const *names,
    TaidaAddonValueV1 *const *values,
    size_t len);
typedef void (*TaidaHostValueRelease)(const struct TaidaHostV1 *host, TaidaAddonValueV1 *value);
typedef TaidaAddonErrorV1 *(*TaidaHostErrorNew)(
    const struct TaidaHostV1 *host, uint32_t code, const uint8_t *msg, size_t msg_len);
typedef void (*TaidaHostErrorRelease)(const struct TaidaHostV1 *host, TaidaAddonErrorV1 *error);

/* TaidaHostV1 (repr C) */
typedef struct TaidaHostV1 {
    uint32_t                abi_version;
    uint32_t                _reserved;
    TaidaHostValueNewUnit   value_new_unit;
    TaidaHostValueNewInt    value_new_int;
    TaidaHostValueNewFlt    value_new_float;
    TaidaHostValueNewBool   value_new_bool;
    TaidaHostValueNewBytes  value_new_str;
    TaidaHostValueNewBytes  value_new_bytes;
    TaidaHostValueNewList   value_new_list;
    TaidaHostValueNewPack   value_new_pack;
    TaidaHostValueRelease   value_release;
    TaidaHostErrorNew       error_new;
    TaidaHostErrorRelease   error_release;
} TaidaHostV1;

/* TaidaAddonDescriptorV1 (repr C, 40 bytes on LP64) */
typedef struct TaidaAddonDescriptorV1 {
    uint32_t                    abi_version;
    uint32_t                    _reserved;
    const char                 *addon_name;
    uint32_t                    function_count;
    uint32_t                    _reserved2;
    const TaidaAddonFunctionV1 *functions;
    TaidaAddonStatusV1         (*init)(const TaidaHostV1 *host);
} TaidaAddonDescriptorV1;

/* Layout drift guards (RC2.5B-003). If any of these fail at compile time,
 * Rust and C side are out of sync and must be reconciled before shipping.
 * LP64 Unix (Linux/macOS) is the only currently supported target; Windows
 * compile smoke test is Phase 3. */
_Static_assert(sizeof(TaidaAddonValueV1)        == 16, "TaidaAddonValueV1 layout drift");
_Static_assert(sizeof(TaidaAddonErrorV1)        == 16, "TaidaAddonErrorV1 layout drift");
_Static_assert(sizeof(TaidaAddonIntPayloadV1)   ==  8, "TaidaAddonIntPayloadV1 layout drift");
_Static_assert(sizeof(TaidaAddonFloatPayloadV1) ==  8, "TaidaAddonFloatPayloadV1 layout drift");
_Static_assert(sizeof(TaidaAddonBytesPayloadV1) == 16, "TaidaAddonBytesPayloadV1 layout drift");
_Static_assert(sizeof(TaidaAddonFunctionV1)     == 24, "TaidaAddonFunctionV1 layout drift");
_Static_assert(sizeof(TaidaAddonDescriptorV1)   == 40, "TaidaAddonDescriptorV1 layout drift");
_Static_assert(sizeof(TaidaHostV1)              == 96, "TaidaHostV1 layout drift");
_Static_assert(sizeof(TaidaAddonPackEntryV1)    == 16, "TaidaAddonPackEntryV1 layout drift");
_Static_assert(sizeof(TaidaAddonPackPayloadV1)  == 16, "TaidaAddonPackPayloadV1 layout drift");

/* ABI version — must match crates/addon-rs/src/abi.rs::TAIDA_ADDON_ABI_VERSION */
#define TAIDA_ADDON_ABI_VERSION_V1 1u

/* Entry symbol name — must match crates/addon-rs/src/abi.rs::TAIDA_ADDON_ENTRY_SYMBOL */
#define TAIDA_ADDON_ENTRY_SYMBOL_V1 "taida_addon_get_v1"

/* ---------------- Host callbacks ---------------- */
/* These are the host-side implementations of TaidaHostV1. Addons call them
 * via the vtable passed into `init`. All allocations use malloc/free so that
 * the host is the single owner (RC1 Phase 3 Lock). */

static TaidaAddonValueV1 *taida_addon_host_value_alloc(uint32_t tag, void *payload) {
    TaidaAddonValueV1 *v = (TaidaAddonValueV1 *)TAIDA_MALLOC(sizeof(TaidaAddonValueV1), "addon_value");
    v->tag = tag;
    v->_reserved = 0;
    v->payload = payload;
    return v;
}

static TaidaAddonValueV1 *taida_addon_host_new_unit(const TaidaHostV1 *host) {
    (void)host;
    return taida_addon_host_value_alloc(TAIDA_ADDON_TAG_UNIT, NULL);
}

static TaidaAddonValueV1 *taida_addon_host_new_int(const TaidaHostV1 *host, int64_t v) {
    (void)host;
    TaidaAddonIntPayloadV1 *p =
        (TaidaAddonIntPayloadV1 *)TAIDA_MALLOC(sizeof(TaidaAddonIntPayloadV1), "addon_int");
    p->value = v;
    return taida_addon_host_value_alloc(TAIDA_ADDON_TAG_INT, p);
}

static TaidaAddonValueV1 *taida_addon_host_new_float(const TaidaHostV1 *host, double v) {
    (void)host;
    TaidaAddonFloatPayloadV1 *p =
        (TaidaAddonFloatPayloadV1 *)TAIDA_MALLOC(sizeof(TaidaAddonFloatPayloadV1), "addon_float");
    p->value = v;
    return taida_addon_host_value_alloc(TAIDA_ADDON_TAG_FLOAT, p);
}

static TaidaAddonValueV1 *taida_addon_host_new_bool(const TaidaHostV1 *host, uint8_t v) {
    (void)host;
    TaidaAddonBoolPayloadV1 *p =
        (TaidaAddonBoolPayloadV1 *)TAIDA_MALLOC(sizeof(TaidaAddonBoolPayloadV1), "addon_bool");
    p->value = v ? 1u : 0u;
    return taida_addon_host_value_alloc(TAIDA_ADDON_TAG_BOOL, p);
}

static TaidaAddonValueV1 *taida_addon_host_new_str(const TaidaHostV1 *host,
                                                    const uint8_t *bytes, size_t len) {
    (void)host;
    TaidaAddonBytesPayloadV1 *p =
        (TaidaAddonBytesPayloadV1 *)TAIDA_MALLOC(sizeof(TaidaAddonBytesPayloadV1), "addon_str");
    uint8_t *buf = (uint8_t *)TAIDA_MALLOC(len == 0 ? 1 : len, "addon_str_buf");
    if (len > 0) memcpy(buf, bytes, len);
    p->ptr = buf;
    p->len = len;
    return taida_addon_host_value_alloc(TAIDA_ADDON_TAG_STR, p);
}

static TaidaAddonValueV1 *taida_addon_host_new_bytes(const TaidaHostV1 *host,
                                                      const uint8_t *bytes, size_t len) {
    (void)host;
    TaidaAddonBytesPayloadV1 *p =
        (TaidaAddonBytesPayloadV1 *)TAIDA_MALLOC(sizeof(TaidaAddonBytesPayloadV1), "addon_bytes");
    uint8_t *buf = (uint8_t *)TAIDA_MALLOC(len == 0 ? 1 : len, "addon_bytes_buf");
    if (len > 0) memcpy(buf, bytes, len);
    p->ptr = buf;
    p->len = len;
    return taida_addon_host_value_alloc(TAIDA_ADDON_TAG_BYTES, p);
}

static TaidaAddonValueV1 *taida_addon_host_new_list(const TaidaHostV1 *host,
                                                     TaidaAddonValueV1 *const *items, size_t len) {
    (void)host;
    TaidaAddonListPayloadV1 *p =
        (TaidaAddonListPayloadV1 *)TAIDA_MALLOC(sizeof(TaidaAddonListPayloadV1), "addon_list");
    TaidaAddonValueV1 **copy = NULL;
    if (len > 0) {
        copy = (TaidaAddonValueV1 **)TAIDA_MALLOC(
            taida_safe_mul(len, sizeof(TaidaAddonValueV1 *), "addon_list_items"),
            "addon_list_items");
        for (size_t i = 0; i < len; i++) copy[i] = items[i];
    }
    p->items = copy;
    p->len = len;
    return taida_addon_host_value_alloc(TAIDA_ADDON_TAG_LIST, p);
}

static TaidaAddonValueV1 *taida_addon_host_new_pack(
    const TaidaHostV1 *host,
    const char *const *names,
    TaidaAddonValueV1 *const *values,
    size_t len)
{
    (void)host;
    TaidaAddonPackPayloadV1 *p =
        (TaidaAddonPackPayloadV1 *)TAIDA_MALLOC(sizeof(TaidaAddonPackPayloadV1), "addon_pack");
    TaidaAddonPackEntryV1 *entries = NULL;
    if (len > 0) {
        entries = (TaidaAddonPackEntryV1 *)TAIDA_MALLOC(
            taida_safe_mul(len, sizeof(TaidaAddonPackEntryV1), "addon_pack_entries"),
            "addon_pack_entries");
        for (size_t i = 0; i < len; i++) {
            size_t nlen = strlen(names[i]);
            char *name_copy = (char *)TAIDA_MALLOC(nlen + 1, "addon_pack_name");
            memcpy(name_copy, names[i], nlen + 1);
            entries[i].name = name_copy;
            entries[i].value = values[i];
        }
    }
    p->entries = entries;
    p->len = len;
    return taida_addon_host_value_alloc(TAIDA_ADDON_TAG_PACK, p);
}

static void taida_addon_host_value_release_inner(TaidaAddonValueV1 *value) {
    if (value == NULL) return;
    switch (value->tag) {
        case TAIDA_ADDON_TAG_UNIT:
            break;
        case TAIDA_ADDON_TAG_INT:
        case TAIDA_ADDON_TAG_FLOAT:
        case TAIDA_ADDON_TAG_BOOL:
            free(value->payload);
            break;
        case TAIDA_ADDON_TAG_STR:
        case TAIDA_ADDON_TAG_BYTES: {
            TaidaAddonBytesPayloadV1 *p = (TaidaAddonBytesPayloadV1 *)value->payload;
            if (p != NULL) {
                free((void *)p->ptr);
                free(p);
            }
            break;
        }
        case TAIDA_ADDON_TAG_LIST: {
            TaidaAddonListPayloadV1 *p = (TaidaAddonListPayloadV1 *)value->payload;
            if (p != NULL) {
                for (size_t i = 0; i < p->len; i++) {
                    taida_addon_host_value_release_inner(p->items[i]);
                }
                free(p->items);
                free(p);
            }
            break;
        }
        case TAIDA_ADDON_TAG_PACK: {
            TaidaAddonPackPayloadV1 *p = (TaidaAddonPackPayloadV1 *)value->payload;
            if (p != NULL) {
                for (size_t i = 0; i < p->len; i++) {
                    free((void *)p->entries[i].name);
                    taida_addon_host_value_release_inner(p->entries[i].value);
                }
                free(p->entries);
                free(p);
            }
            break;
        }
        default:
            break;
    }
    free(value);
}

static void taida_addon_host_value_release(const TaidaHostV1 *host, TaidaAddonValueV1 *value) {
    (void)host;
    taida_addon_host_value_release_inner(value);
}

static TaidaAddonErrorV1 *taida_addon_host_error_new(
    const TaidaHostV1 *host, uint32_t code, const uint8_t *msg, size_t msg_len)
{
    (void)host;
    TaidaAddonErrorV1 *err =
        (TaidaAddonErrorV1 *)TAIDA_MALLOC(sizeof(TaidaAddonErrorV1), "addon_error");
    char *copy = (char *)TAIDA_MALLOC(msg_len + 1, "addon_error_msg");
    if (msg_len > 0) memcpy(copy, msg, msg_len);
    copy[msg_len] = '\0';
    err->code = code;
    err->_reserved = 0;
    err->message = copy;
    return err;
}

static void taida_addon_host_error_release(const TaidaHostV1 *host, TaidaAddonErrorV1 *error) {
    (void)host;
    if (error == NULL) return;
    free((void *)error->message);
    free(error);
}

/* Global host vtable. Initialised lazily on first call. */
static TaidaHostV1 taida_addon_host_table = {
    .abi_version    = TAIDA_ADDON_ABI_VERSION_V1,
    ._reserved      = 0,
    .value_new_unit = taida_addon_host_new_unit,
    .value_new_int  = taida_addon_host_new_int,
    .value_new_float= taida_addon_host_new_float,
    .value_new_bool = taida_addon_host_new_bool,
    .value_new_str  = taida_addon_host_new_str,
    .value_new_bytes= taida_addon_host_new_bytes,
    .value_new_list = taida_addon_host_new_list,
    .value_new_pack = taida_addon_host_new_pack,
    .value_release  = taida_addon_host_value_release,
    .error_new      = taida_addon_host_error_new,
    .error_release  = taida_addon_host_error_release,
};

/* ---------------- Addon registry ---------------- */

#define TAIDA_ADDON_MAX 16

/* RC2.5-3c: Platform abstraction for dlopen / dlsym / dlclose.
 *
 * Frozen design (.dev/RC2_5_DESIGN.md §A-6 / RC2.5B-005):
 *   Linux + macOS use dlfcn.h (already included earlier in the file).
 *   Windows uses LoadLibraryA / GetProcAddress / FreeLibrary.
 *
 * v1 scope: Linux primary, macOS secondary, Windows compile smoke test
 * only. Real Windows execution testing is RC3+.
 *
 * Note: the rest of native_runtime.c is currently Unix-only via direct
 * dlfcn.h usage in the OpenSSL / quiche blocks. This abstraction lives
 * inside the RC2.5 addon dispatch block so a future Windows port can
 * reuse it without disturbing the existing Unix-only code paths. */
#ifdef _WIN32
#  include <windows.h>
   typedef HMODULE  taida_dl_handle_t;
#  define TAIDA_DL_OPEN(path)    LoadLibraryA(path)
#  define TAIDA_DL_SYM(h, sym)   ((void *)GetProcAddress((h), (sym)))
#  define TAIDA_DL_CLOSE(h)      FreeLibrary(h)
#  define TAIDA_DL_ERROR_CLEAR()  ((void)0)
   /* GetLastError is numeric on Windows; we render it as a short fallback
    * string when dlerror-style lookups are not available. */
   static const char *taida_dl_error(void) {
       static char taida_dl_err_buf[64];
       snprintf(taida_dl_err_buf, sizeof(taida_dl_err_buf),
                "Windows dynamic load error code %lu", (unsigned long)GetLastError());
       return taida_dl_err_buf;
   }
#  define TAIDA_DL_ERROR()       taida_dl_error()
#else
   typedef void *taida_dl_handle_t;
#  define TAIDA_DL_OPEN(path)    dlopen((path), RTLD_NOW | RTLD_LOCAL)
#  define TAIDA_DL_SYM(h, sym)   dlsym((h), (sym))
#  define TAIDA_DL_CLOSE(h)      dlclose(h)
#  define TAIDA_DL_ERROR_CLEAR() ((void)dlerror())
#  define TAIDA_DL_ERROR()       dlerror()
#endif

typedef struct {
    const char                   *package_id;     /* strdup, owned */
    taida_dl_handle_t             dl_handle;      /* platform handle */
    const TaidaAddonDescriptorV1 *descriptor;
    int                           init_done;
} TaidaAddonEntry;

static TaidaAddonEntry taida_addon_registry[TAIDA_ADDON_MAX];
static size_t          taida_addon_registry_len = 0;
static pthread_mutex_t taida_addon_registry_mu  = PTHREAD_MUTEX_INITIALIZER;

/* RC2.5-3b: Hard-fail entry for dlopen / dlsym / ABI / init failures.
 *
 * Frozen contract (.dev/RC2_5_IMPL_SPEC.md F-7):
 *   - dlopen / dlsym / ABI mismatch / init failure are *all* hard fail
 *   - format: "taida: addon load failed: <package_id>: <detail>\n"
 *   - never converted to a Taida throw (the caller has no chance to
 *     catch a failure that happens before the addon is even loaded)
 *
 * Distinct from Status::Error, which RC2.5-3a converts to a catchable
 * Taida error variant via taida_addon_throw_call_error below.
 *
 * RC2.5B-004 (Phase 4): also emit a second "hint" line explaining that
 * cdylib paths are resolved at build time and RC2.5 v1 does not do a
 * runtime rescan. This is the documented known constraint — developers
 * who move a `.so` after build get immediate feedback telling them why
 * it failed and where to look. The hint line is additive (does not
 * replace the existing detail line) so Phase 3 tests that assert the
 * presence of `taida: addon load failed:` continue to pass. */
static void taida_addon_fail(const char *pkg, const char *detail) __attribute__((noreturn));
static void taida_addon_fail(const char *pkg, const char *detail) {
    fprintf(stderr, "taida: addon load failed: %s: %s\n",
            pkg ? pkg : "(unknown)", detail ? detail : "(unknown)");
    fprintf(stderr,
            "taida: hint: cdylib path was resolved at build time; "
            "RC2.5 v1 does not re-search at runtime "
            "(see .dev/RC2_5_BLOCKERS.md::RC2.5B-004)\n");
    exit(1);
}

/* RC2.5-3a: Build a Taida `AddonError` pack and longjmp out via
 * taida_throw. This is the deterministic "addon returned Status::Error"
 * path; the user can catch it with `|== AddonError` (typed) or `|==
 * Error` (catch-all) just like any other Taida runtime error.
 *
 * The pack shape mirrors what the interpreter produces in
 * `src/interpreter/addon_eval.rs::try_addon_func` so backend parity
 * holds (RC2.5-4b will pin this byte-for-byte).
 *
 * Never returns (taida_throw longjmps to the nearest error ceiling, or
 * gorilla-fails the process if there is none). */
static void taida_addon_throw_call_error(const char *function_name,
                                          uint32_t code,
                                          const char *message) __attribute__((noreturn));
static void taida_addon_throw_call_error(const char *function_name,
                                          uint32_t code,
                                          const char *message) {
    /* Defensive nulls — the addon may legitimately omit a message. */
    const char *fn  = function_name ? function_name : "(unknown)";
    const char *msg = message       ? message       : "addon returned Status::Error";

    /* Compose a single human-readable string that matches the
     * interpreter's `AddonCallError::AddonError` Display impl shape:
     *   "addon call failed: '<addon>::<fn>' returned error code=N message='...'"
     * We don't have the addon name handy here (only the function), so
     * we compose without the addon prefix; the test surface keys off
     * the type name (`AddonError`) and the message substring. */
    char composed[1024];
    snprintf(composed, sizeof(composed),
             "addon call failed: '%s' returned error code=%u message='%s'",
             fn, (unsigned)code, msg);

    /* taida_make_error builds a Pack with `type` / `message` / `__type`
     * fields, which is exactly the shape `taida_error_type_matches`
     * expects for `|== e: AddonError` handler matching. */
    taida_val err = taida_make_error("AddonError", composed);
    taida_throw(err);
    /* unreachable */
    abort();
}

/* Lookup or load. Caller does NOT hold the mutex.
 * On return, the entry is fully initialised (handshake + init done). */
static TaidaAddonEntry *taida_addon_ensure_loaded(
    const char *package_id, const char *cdylib_path)
{
    pthread_mutex_lock(&taida_addon_registry_mu);

    /* Linear scan for existing entry (RC2.5B-006: small N, fine). */
    for (size_t i = 0; i < taida_addon_registry_len; i++) {
        if (strcmp(taida_addon_registry[i].package_id, package_id) == 0) {
            TaidaAddonEntry *entry = &taida_addon_registry[i];
            pthread_mutex_unlock(&taida_addon_registry_mu);
            return entry;
        }
    }

    if (taida_addon_registry_len >= TAIDA_ADDON_MAX) {
        pthread_mutex_unlock(&taida_addon_registry_mu);
        taida_addon_fail(package_id, "addon limit exceeded (TAIDA_ADDON_MAX)");
    }

    /* Reserve a slot before unlocking. We keep the mutex held during dlopen
     * for simplicity; dlopen is idempotent per handle in glibc so nested
     * loads via init() would not deadlock on the dynamic linker itself,
     * but we still invoke the addon's init() AFTER releasing our lock to
     * avoid re-entrancy on taida_addon_registry_mu. */
    TaidaAddonEntry *entry = &taida_addon_registry[taida_addon_registry_len];
    entry->package_id = strdup(package_id);
    if (entry->package_id == NULL) {
        pthread_mutex_unlock(&taida_addon_registry_mu);
        taida_addon_fail(package_id, "strdup OOM");
    }
    entry->dl_handle = NULL;
    entry->descriptor = NULL;
    entry->init_done = 0;

    /* RC2.5-3c: platform-abstracted dynamic load. On Linux/macOS this is
     * dlopen(RTLD_NOW | RTLD_LOCAL); on Windows it is LoadLibraryA. The
     * cdylib_path was resolved at build time by lower.rs so it is an
     * absolute path with no environment lookup happening here. */
    taida_dl_handle_t handle = TAIDA_DL_OPEN(cdylib_path);
    if (handle == NULL) {
        const char *err = TAIDA_DL_ERROR();
        pthread_mutex_unlock(&taida_addon_registry_mu);
        taida_addon_fail(package_id,
            err ? err : "dynamic load failed (cdylib path was resolved at build time)");
    }
    entry->dl_handle = handle;

    /* On Linux/macOS, dlerror() returns NULL when no error has been
     * signaled since the previous call. On Windows, GetProcAddress
     * returns NULL on failure and our taida_dl_error fallback always
     * returns a non-empty diagnostic, so we only consult it when the
     * symbol pointer itself is NULL. */
    TAIDA_DL_ERROR_CLEAR();
    void *sym = TAIDA_DL_SYM(handle, TAIDA_ADDON_ENTRY_SYMBOL_V1);
    if (sym == NULL) {
        const char *sym_err = TAIDA_DL_ERROR();
        pthread_mutex_unlock(&taida_addon_registry_mu);
        taida_addon_fail(package_id, sym_err ? sym_err : "entry symbol not found");
    }

    typedef const TaidaAddonDescriptorV1 *(*TaidaAddonGetV1)(void);
    TaidaAddonGetV1 get_fn = (TaidaAddonGetV1)sym;
    const TaidaAddonDescriptorV1 *desc = get_fn();
    if (desc == NULL) {
        pthread_mutex_unlock(&taida_addon_registry_mu);
        taida_addon_fail(package_id, "descriptor is null");
    }
    if (desc->abi_version != TAIDA_ADDON_ABI_VERSION_V1) {
        pthread_mutex_unlock(&taida_addon_registry_mu);
        taida_addon_fail(package_id, "ABI version mismatch");
    }
    entry->descriptor = desc;
    taida_addon_registry_len += 1;

    pthread_mutex_unlock(&taida_addon_registry_mu);

    /* Call init outside the lock. Racing callers for the same package would
     * have blocked above on the first lookup; the second caller observes
     * init_done already set. We use a compare-and-swap style with a second
     * lock acquisition. */
    pthread_mutex_lock(&taida_addon_registry_mu);
    if (!entry->init_done) {
        if (desc->init != NULL) {
            TaidaAddonStatusV1 init_status = desc->init(&taida_addon_host_table);
            if (init_status != TAIDA_ADDON_STATUS_OK) {
                pthread_mutex_unlock(&taida_addon_registry_mu);
                taida_addon_fail(package_id, "init callback returned non-Ok");
            }
        }
        entry->init_done = 1;
    }
    pthread_mutex_unlock(&taida_addon_registry_mu);

    return entry;
}

/* ---------------- Value bridge (Taida Pack ↔ addon ABI v1) ---------------- */

/* Convert a single taida runtime boxed value into an addon ABI Value.
 * `raw` is the raw taida_val as stored in the pack cell; `tag` is the
 * TAIDA_TAG_* (runtime internal) tag. Stack-allocated; caller must keep
 * stable until the call returns. */
static void taida_addon_val_from_raw(
    taida_val raw, taida_val internal_tag,
    TaidaAddonValueV1 *out,
    TaidaAddonIntPayloadV1 *int_scratch,
    TaidaAddonBytesPayloadV1 *str_scratch,
    TaidaAddonBoolPayloadV1 *bool_scratch)
{
    out->_reserved = 0;
    switch (internal_tag) {
        case TAIDA_TAG_INT:
            int_scratch->value = (int64_t)raw;
            out->tag = TAIDA_ADDON_TAG_INT;
            out->payload = int_scratch;
            return;
        case TAIDA_TAG_BOOL:
            bool_scratch->value = raw ? 1u : 0u;
            out->tag = TAIDA_ADDON_TAG_BOOL;
            out->payload = bool_scratch;
            return;
        case TAIDA_TAG_STR: {
            const char *s = (const char *)(taida_ptr)raw;
            str_scratch->ptr = (const uint8_t *)(s ? s : "");
            str_scratch->len = s ? strlen(s) : 0;
            out->tag = TAIDA_ADDON_TAG_STR;
            out->payload = str_scratch;
            return;
        }
        default:
            /* Unit / unsupported — carry across as Unit so the addon can
             * reject with UNSUPPORTED_VALUE if it matters. Phase 1 scope. */
            out->tag = TAIDA_ADDON_TAG_UNIT;
            out->payload = NULL;
            return;
    }
}

/* Convert an addon ABI Value back into a taida runtime boxed value.
 * Phase 1 scope: Int / Bool / Str / Unit / Pack-of-scalars.
 * Caller is responsible for releasing the source value afterwards. */
static taida_val taida_addon_val_to_raw(const TaidaAddonValueV1 *v) {
    if (v == NULL) return 0;
    switch (v->tag) {
        case TAIDA_ADDON_TAG_UNIT:
            return 0;
        case TAIDA_ADDON_TAG_INT: {
            const TaidaAddonIntPayloadV1 *p = (const TaidaAddonIntPayloadV1 *)v->payload;
            return (taida_val)(p ? p->value : 0);
        }
        case TAIDA_ADDON_TAG_BOOL: {
            const TaidaAddonBoolPayloadV1 *p = (const TaidaAddonBoolPayloadV1 *)v->payload;
            return (taida_val)(p && p->value ? 1 : 0);
        }
        case TAIDA_ADDON_TAG_STR: {
            const TaidaAddonBytesPayloadV1 *p = (const TaidaAddonBytesPayloadV1 *)v->payload;
            if (p == NULL) return (taida_val)taida_str_new_copy("");
            /* taida_str_new copies into a taida-managed allocation. */
            char *tmp = (char *)TAIDA_MALLOC(p->len + 1, "addon_str_tmp");
            if (p->len > 0) memcpy(tmp, p->ptr, p->len);
            tmp[p->len] = '\0';
            taida_val r = (taida_val)taida_str_new_copy(tmp);
            free(tmp);
            return r;
        }
        case TAIDA_ADDON_TAG_PACK: {
            /* Minimal pack marshalling: each field becomes a raw int/str entry.
             * Uses hash-indexed storage compatible with the runtime pack. */
            const TaidaAddonPackPayloadV1 *p = (const TaidaAddonPackPayloadV1 *)v->payload;
            if (p == NULL || p->len == 0) return (taida_val)taida_pack_new(0);
            taida_val pack = (taida_val)taida_pack_new((taida_val)p->len);
            for (size_t i = 0; i < p->len; i++) {
                taida_val field_hash = taida_str_hash((taida_ptr)p->entries[i].name);
                taida_pack_set_hash((taida_ptr)pack, (taida_val)i, field_hash);
                TaidaAddonValueV1 *child = p->entries[i].value;
                if (child == NULL) {
                    taida_pack_set((taida_ptr)pack, (taida_val)i, 0);
                    taida_pack_set_tag((taida_ptr)pack, (taida_val)i, TAIDA_TAG_INT);
                    continue;
                }
                switch (child->tag) {
                    case TAIDA_ADDON_TAG_INT: {
                        const TaidaAddonIntPayloadV1 *ip = (const TaidaAddonIntPayloadV1 *)child->payload;
                        taida_pack_set((taida_ptr)pack, (taida_val)i, (taida_val)(ip ? ip->value : 0));
                        taida_pack_set_tag((taida_ptr)pack, (taida_val)i, TAIDA_TAG_INT);
                        break;
                    }
                    case TAIDA_ADDON_TAG_BOOL: {
                        const TaidaAddonBoolPayloadV1 *bp = (const TaidaAddonBoolPayloadV1 *)child->payload;
                        taida_pack_set((taida_ptr)pack, (taida_val)i, (taida_val)(bp && bp->value ? 1 : 0));
                        taida_pack_set_tag((taida_ptr)pack, (taida_val)i, TAIDA_TAG_BOOL);
                        break;
                    }
                    case TAIDA_ADDON_TAG_STR: {
                        const TaidaAddonBytesPayloadV1 *sp = (const TaidaAddonBytesPayloadV1 *)child->payload;
                        if (sp == NULL) {
                            taida_pack_set((taida_ptr)pack, (taida_val)i, (taida_val)taida_str_new_copy(""));
                        } else {
                            char *tmp = (char *)TAIDA_MALLOC(sp->len + 1, "addon_pack_str_tmp");
                            if (sp->len > 0) memcpy(tmp, sp->ptr, sp->len);
                            tmp[sp->len] = '\0';
                            taida_pack_set((taida_ptr)pack, (taida_val)i, (taida_val)taida_str_new_copy(tmp));
                            free(tmp);
                        }
                        taida_pack_set_tag((taida_ptr)pack, (taida_val)i, TAIDA_TAG_STR);
                        break;
                    }
                    default:
                        taida_pack_set((taida_ptr)pack, (taida_val)i, 0);
                        taida_pack_set_tag((taida_ptr)pack, (taida_val)i, TAIDA_TAG_INT);
                        break;
                }
            }
            return pack;
        }
        default:
            return 0;
    }
}

/* ---------------- Dispatcher ---------------- */

/* Called by Cranelift-lowered code at each addon function invocation.
 *   package_id    — static C string in .rodata (addon package id)
 *   cdylib_path   — absolute path resolved at build time
 *   function_name — function identifier as listed in addon.toml
 *   argc          — number of arguments (must match descriptor entry)
 *   argv_pack     — Taida Pack used as an argv carrier: fields 0..argc-1
 *                   hold positional arguments, tagged with TAIDA_TAG_*.
 *                   argc == 0 allows passing 0 here.
 * Returns a taida_val carrying the addon return value. */
int64_t taida_addon_call(
    const char *package_id,
    const char *cdylib_path,
    const char *function_name,
    int64_t     argc,
    int64_t     argv_pack)
{
    if (package_id == NULL || cdylib_path == NULL || function_name == NULL) {
        taida_addon_fail(package_id, "null pointer in taida_addon_call");
    }

    TaidaAddonEntry *entry = taida_addon_ensure_loaded(package_id, cdylib_path);
    const TaidaAddonDescriptorV1 *desc = entry->descriptor;

    /* Linear scan for the function (RC2.5B-006). */
    const TaidaAddonFunctionV1 *fn = NULL;
    for (uint32_t i = 0; i < desc->function_count; i++) {
        if (strcmp(desc->functions[i].name, function_name) == 0) {
            fn = &desc->functions[i];
            break;
        }
    }
    if (fn == NULL) {
        char detail[256];
        snprintf(detail, sizeof(detail), "function '%s' not found", function_name);
        taida_addon_fail(package_id, detail);
    }
    if ((int64_t)fn->arity != argc) {
        char detail[256];
        snprintf(detail, sizeof(detail),
                 "arity mismatch for '%s': declared %u, got %lld",
                 function_name, (unsigned)fn->arity, (long long)argc);
        taida_addon_fail(package_id, detail);
    }

    /* Marshal argv. Up to 16 scalars inline on the stack; larger payloads
     * fall back to heap allocation. */
    TaidaAddonValueV1 inline_values[16];
    TaidaAddonIntPayloadV1 inline_ints[16];
    TaidaAddonBytesPayloadV1 inline_strs[16];
    TaidaAddonBoolPayloadV1 inline_bools[16];
    TaidaAddonValueV1 *values_ptr = inline_values;
    TaidaAddonIntPayloadV1 *ints_ptr = inline_ints;
    TaidaAddonBytesPayloadV1 *strs_ptr = inline_strs;
    TaidaAddonBoolPayloadV1 *bools_ptr = inline_bools;
    int heap_allocated = 0;
    if (argc > 16) {
        values_ptr = (TaidaAddonValueV1 *)TAIDA_MALLOC(
            taida_safe_mul((size_t)argc, sizeof(TaidaAddonValueV1), "addon_argv"),
            "addon_argv");
        ints_ptr = (TaidaAddonIntPayloadV1 *)TAIDA_MALLOC(
            taida_safe_mul((size_t)argc, sizeof(TaidaAddonIntPayloadV1), "addon_argv_int"),
            "addon_argv_int");
        strs_ptr = (TaidaAddonBytesPayloadV1 *)TAIDA_MALLOC(
            taida_safe_mul((size_t)argc, sizeof(TaidaAddonBytesPayloadV1), "addon_argv_str"),
            "addon_argv_str");
        bools_ptr = (TaidaAddonBoolPayloadV1 *)TAIDA_MALLOC(
            taida_safe_mul((size_t)argc, sizeof(TaidaAddonBoolPayloadV1), "addon_argv_bool"),
            "addon_argv_bool");
        heap_allocated = 1;
    }

    if (argc > 0) {
        taida_val *pack = (taida_val *)(taida_ptr)argv_pack;
        if (pack == NULL) {
            if (heap_allocated) {
                free(values_ptr); free(ints_ptr); free(strs_ptr); free(bools_ptr);
            }
            taida_addon_fail(package_id, "argv pack is null");
        }
        /* Pack internal layout: [magic+rc, count, hash0, tag0, val0, hash1, tag1, val1, ...] */
        for (int64_t i = 0; i < argc; i++) {
            taida_val tag = pack[2 + i * 3 + 1];
            taida_val raw = pack[2 + i * 3 + 2];
            taida_addon_val_from_raw(raw, tag, &values_ptr[i],
                                     &ints_ptr[i], &strs_ptr[i], &bools_ptr[i]);
        }
    }

    TaidaAddonValueV1 *out_value = NULL;
    TaidaAddonErrorV1 *out_error = NULL;
    TaidaAddonStatusV1 status = fn->call(values_ptr, (uint32_t)argc, &out_value, &out_error);

    /* Free argv scratch eagerly so the upcoming taida_throw branch (which
     * longjmps and never returns) does not leak the heap-allocated
     * fallback buffers when argc > 16. The inline (stack) buffers are
     * always reclaimed by stack unwinding regardless of which path we
     * take below. */
    if (heap_allocated) {
        free(values_ptr); free(ints_ptr); free(strs_ptr); free(bools_ptr);
        heap_allocated = 0;
    }

    taida_val result = 0;
    if (status == TAIDA_ADDON_STATUS_OK) {
        /* Defensive: addon may have written to out_error even on success.
         * Release it so we don't leak. */
        if (out_error != NULL) {
            taida_addon_host_error_release(&taida_addon_host_table, out_error);
            out_error = NULL;
        }
        if (out_value != NULL) {
            result = taida_addon_val_to_raw(out_value);
            taida_addon_host_value_release(&taida_addon_host_table, out_value);
        }
        return (int64_t)result;
    }

    /* RC2.5-3a: Status::Error with an out_error → catchable Taida
     * AddonError variant. Mirrors the interpreter's behaviour in
     * `src/interpreter/addon_eval.rs::try_addon_func`, which wraps an
     * `AddonCallError::AddonError { code, message }` into a
     * `Signal::Throw(Value::Error(ErrorValue { error_type:
     * "AddonError", ... }))`.
     *
     * Other non-Ok statuses (ArityMismatch / InvalidState / etc.) also
     * route through here so the Taida user surface is uniform — the
     * dispatcher already validates arity at the C level above, so the
     * remaining non-Ok variants from a real addon are deterministic
     * addon-side bugs that the user can still catch via `|== Error`. */

    /* Snapshot the error message into a stack buffer so we can release
     * the host-owned out_error / out_value before the longjmp (which
     * skips ordinary scope cleanup). */
    char message_buf[512];
    uint32_t err_code = (uint32_t)status;
    if (out_error != NULL) {
        if (out_error->message != NULL) {
            snprintf(message_buf, sizeof(message_buf), "%s", out_error->message);
        } else {
            snprintf(message_buf, sizeof(message_buf), "addon returned error (no message)");
        }
        err_code = out_error->code;
        taida_addon_host_error_release(&taida_addon_host_table, out_error);
        out_error = NULL;
    } else {
        /* Status::Error with no out_error, or one of the typed
         * non-Ok statuses. */
        const char *status_name = "addon call failed";
        switch (status) {
            case TAIDA_ADDON_STATUS_ERROR:
                status_name = "addon returned Status::Error without out_error";
                break;
            case TAIDA_ADDON_STATUS_ABI_MISMATCH:
                status_name = "addon returned Status::AbiMismatch";
                break;
            case TAIDA_ADDON_STATUS_INVALID_STATE:
                status_name = "addon returned Status::InvalidState";
                break;
            case TAIDA_ADDON_STATUS_UNSUPPORTED_VALUE:
                status_name = "addon returned Status::UnsupportedValue";
                break;
            case TAIDA_ADDON_STATUS_NULL_POINTER:
                status_name = "addon returned Status::NullPointer";
                break;
            case TAIDA_ADDON_STATUS_ARITY_MISMATCH:
                status_name = "addon returned Status::ArityMismatch";
                break;
            default:
                break;
        }
        snprintf(message_buf, sizeof(message_buf), "%s", status_name);
    }

    /* Defensive: release any value slot the addon might also have filled. */
    if (out_value != NULL) {
        taida_addon_host_value_release(&taida_addon_host_table, out_value);
        out_value = NULL;
    }

    /* Hand off to the Taida error path. taida_addon_throw_call_error
     * never returns — it longjmps via taida_throw to the nearest
     * gorilla ceiling, or hard-fails the process (with a different
     * "Runtime error: ..." prefix from taida_throw, not the
     * "addon load failed" prefix used for dlopen failures) if no
     * ceiling is on the stack. */
    taida_addon_throw_call_error(function_name, err_code, message_buf);
    /* unreachable */
    return 0;
}

/* ============================================================================ */
/* end of RC2.5 addon dispatch block                                            */
/* ============================================================================ */

int main(int argc, char **argv) {
    taida_cli_argc = argc;
    taida_cli_argv = argv;
    /* C26B-021 (Option B): force line-buffered stdio so that native output
     * timing matches Interpreter (Rust println!) / JS (Node console.log) when
     * the process is attached to a pipe. POSIX libc defaults stdout to
     * fully buffered (4KB/8KB) when stdout is not a tty, which broke
     * 3-backend observability parity for curl-driven HTTP trace logs:
     * Interpreter/JS emitted trace-per-request in real time, but Native
     * buffered them until server shutdown. setvbuf at process entry
     * restores line-buffered semantics everywhere with a single init call
     * (lower overhead than per-call fflush). _IOLBF flushes on every '\n'
     * and at stdio close, which matches the observable behaviour of the
     * other two backends. */
    setvbuf(stdout, NULL, _IOLBF, 0);
    setvbuf(stderr, NULL, _IOLBF, 0);
    /* C12-5 (FB-18): `_taida_main` now returns whatever the final expression
     * evaluates to — in particular `stdout(...)` returns the byte count (Int)
     * instead of Unit. Leaking that value into the process exit code would
     * make `./program` exit non-zero for trivial `stdout("hi")` programs.
     * Drop the return value and exit 0 for a clean run. Taida programs that
     * want a custom exit code call `exit(n)` explicitly (no-return path). */
    (void)_taida_main();
    return 0;
}
