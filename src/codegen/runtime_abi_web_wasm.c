/**
 * runtime_abi_web_wasm.c -- shared WebRequest/WebResponse ABI for WASM profiles
 *
 * This runtime fragment is linked only when a module uses `taida-lang/abi`
 * helpers or when build handler mode needs the low-level `taida_abi_web_*`
 * exports. It is shared by wasm-min / wasm-wasi / wasm-edge / wasm-full.
 */

#include <stdint.h>

extern void *wasm_alloc(unsigned int size);
extern int32_t wasm_arena_enter(void);
extern void wasm_arena_leave(int32_t saved);

extern int64_t taida_list_new(void);
extern int64_t taida_list_push(int64_t list, int64_t item);
extern void taida_list_set_elem_tag(int64_t list, int64_t tag);
extern int64_t taida_str_hash(int64_t str_ptr);
extern int64_t taida_pack_new(int64_t field_count);
extern int64_t taida_pack_set_hash(int64_t pack_ptr, int64_t index, int64_t hash);
extern int64_t taida_pack_set_tag(int64_t pack_ptr, int64_t index, int64_t tag);
extern int64_t taida_pack_set(int64_t pack_ptr, int64_t index, int64_t value);
extern int64_t taida_pack_get(int64_t pack_ptr, int64_t field_hash);
extern int64_t taida_json_encode(int64_t value);
extern int64_t taida_json_encode_wire(int64_t value, int64_t schema);
extern int64_t taida_json_schema_cast(int64_t raw_ptr, int64_t schema_ptr);
extern int64_t taida_async_ok(int64_t value);
extern int64_t taida_async_err(int64_t error);
extern int64_t taida_async_pending_with_error(int64_t error);
extern int64_t taida_make_error(int64_t type_ptr, int64_t msg_ptr);

int64_t taida_abi_web_store_error_response_json(int64_t status, int64_t message_ptr);

#define ABI_TAG_INT   0
#define ABI_TAG_STR   3
#define ABI_TAG_PACK  4
#define ABI_TAG_LIST  5
#define ABI_BYTES_MAGIC 0x5441494442595400LL
#define ABI_WASM_LIST_MAGIC 0x544149444C535400LL
#define ABI_WASM_LIST_ELEMS 4
#define TAIDA_ABI_WEB_MAX_REQUEST_BYTES (16 * 1024 * 1024)
#define TAIDA_ABI_WEB_MAX_HEADERS 512
#define TAIDA_ABI_WEB_ALLOC_TABLE_SIZE 64
#define TAIDA_ABI_WEB_OUT_TABLE_SIZE 64

typedef struct {
    int32_t active;
    int32_t state;
    uint32_t generation;
    int32_t ptr;
    int32_t len;
    int32_t arena_mark;
    int32_t request_ptr;
    int32_t request_len;
} TaidaAbiWebOut;

typedef struct {
    int32_t active;
    int32_t ptr;
    int32_t len;
    int32_t arena_mark;
} TaidaAbiWebAlloc;

typedef struct {
    char *buf;
    int32_t len;
    int32_t cap;
} TaidaAbiJsonBuilder;

static TaidaAbiWebAlloc abi_web_allocs[TAIDA_ABI_WEB_ALLOC_TABLE_SIZE];
static int32_t abi_web_alloc_next = 0;
static TaidaAbiWebOut abi_web_outs[TAIDA_ABI_WEB_OUT_TABLE_SIZE];
static int32_t abi_web_out_next = 0;
/* Non-zero only while materializing one handler response. */
static int32_t abi_web_current_arena_mark = 0;
static char *abi_host_pending_json = (char *)0;
static int32_t abi_host_pending_len = 0;
static int64_t abi_host_next_id = 1;
static char abi_host_pending_marker;
static char *abi_host_resume_json = (char *)0;
static int32_t abi_host_resume_len = 0;
static int32_t abi_host_resume_active = 0;

static int abi_wasm_is_readable(int64_t value, uint64_t min_bytes) {
    if (value <= 0) return 0;
    uint64_t start = (uint64_t)value;
    uint64_t mem_bytes = (uint64_t)__builtin_wasm_memory_size(0) * 65536u;
    if (start >= mem_bytes) return 0;
    if (min_bytes > mem_bytes) return 0;
    if (start + min_bytes < start) return 0;
    return start + min_bytes <= mem_bytes;
}

static int32_t abi_strlen(const char *s) {
    int32_t n = 0;
    if (!s) return 0;
    int64_t addr = (int64_t)(intptr_t)s;
    while (n < TAIDA_ABI_WEB_MAX_REQUEST_BYTES &&
           abi_wasm_is_readable(addr + n, 1) &&
           s[n]) {
        n++;
    }
    return n;
}

static void abi_memcpy(void *dest, const void *src, int32_t n) {
    char *d = (char *)dest;
    const char *s = (const char *)src;
    while (n-- > 0) *d++ = *s++;
}

static char *abi_copy_bytes(const char *src, int32_t len) {
    if (len < 0) len = 0;
    char *out = (char *)wasm_alloc((unsigned int)(len + 1));
    if (!out) return (char *)"";
    if (src && len > 0) abi_memcpy(out, src, len);
    out[len] = '\0';
    return out;
}

static char *abi_copy_cstr(const char *src) {
    return abi_copy_bytes(src ? src : "", abi_strlen(src));
}

static int64_t abi_hash_cstr(const char *s) {
    return taida_str_hash((int64_t)(intptr_t)s);
}

static int64_t abi_status_clamp(int64_t status) {
    if (status < 100) return 100;
    if (status > 599) return 599;
    return status;
}

static char *abi_json_parse_string(const char *json, int32_t len, int32_t *p);

static int abi_cstr_eq(const char *a, const char *b) {
    if (!a || !b) return 0;
    int32_t i = 0;
    while (a[i] && b[i]) {
        if (a[i] != b[i]) return 0;
        i++;
    }
    return a[i] == '\0' && b[i] == '\0';
}

static int abi_header_name_valid(const char *name) {
    if (!name || !name[0]) return 0;
    for (int32_t i = 0; name[i]; i++) {
        unsigned char c = (unsigned char)name[i];
        if ((c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') ||
            (c >= '0' && c <= '9') || c == '!' || c == '#' || c == '$' ||
            c == '%' || c == '&' || c == '\'' || c == '*' || c == '+' ||
            c == '-' || c == '.' || c == '^' || c == '_' || c == '`' ||
            c == '|' || c == '~') {
            continue;
        }
        return 0;
    }
    return 1;
}

static int abi_header_value_valid(const char *value) {
    if (!value) return 1;
    for (int32_t i = 0; value[i]; i++) {
        unsigned char c = (unsigned char)value[i];
        if (c == '\r' || c == '\n') return 0;
        if (c < 0x20 && c != '\t') return 0;
    }
    return 1;
}

static int64_t abi_pair_list_new(void) {
    int64_t list = taida_list_new();
    taida_list_set_elem_tag(list, ABI_TAG_PACK);
    return list;
}

static int64_t abi_name_value_pair_new(const char *name, const char *value) {
    char *name_copy = abi_copy_cstr(name ? name : "");
    char *value_copy = abi_copy_cstr(value ? value : "");
    int64_t pack = taida_pack_new(2);
    taida_pack_set_hash(pack, 0, abi_hash_cstr("name"));
    taida_pack_set_tag(pack, 0, ABI_TAG_STR);
    taida_pack_set(pack, 0, (int64_t)(intptr_t)name_copy);
    taida_pack_set_hash(pack, 1, abi_hash_cstr("value"));
    taida_pack_set_tag(pack, 1, ABI_TAG_STR);
    taida_pack_set(pack, 1, (int64_t)(intptr_t)value_copy);
    return pack;
}

static int64_t abi_pair_list_append_raw(int64_t list, const char *name, const char *value) {
    if (!list) list = abi_pair_list_new();
    return taida_list_push(list, abi_name_value_pair_new(name, value));
}

static int64_t abi_header_list_append(int64_t list, const char *name, const char *value) {
    if (!list) list = abi_pair_list_new();
    if (!abi_header_name_valid(name) || !abi_header_value_valid(value)) return list;
    return abi_pair_list_append_raw(list, name, value);
}

/* The response builders store the headers list pointer into each derived
 * response pack without copying, so several response values can share one
 * spine. taida_list_push mutates that spine in place; appending through a
 * shared list would therefore also mutate the *input* response, breaking
 * the documented "returns a new WebResponse, input unchanged" helper
 * contract (interpreter keeps the input intact). Copy the spine before
 * appending — pair packs are immutable and stay shared. WASM layout:
 * [cap, len, elem_tag, magic, items...]. Arena allocation, no retain. */
static int64_t abi_pair_list_copy(int64_t list_ptr) {
    int64_t out = abi_pair_list_new();
    if (!list_ptr) return out;
    int64_t *src = (int64_t *)(intptr_t)list_ptr;
    int64_t len = src[1];
    for (int64_t i = 0; i < len; i++) {
        out = taida_list_push(out, src[4 + i]);
    }
    return out;
}

static int64_t abi_bytes_default(void) {
    int64_t cap = 8;
    int64_t *bytes = (int64_t *)wasm_alloc((unsigned int)((ABI_WASM_LIST_ELEMS + cap + 1) * 8));
    if (!bytes) return 0;
    bytes[0] = cap;
    bytes[1] = 0;
    bytes[2] = ABI_TAG_INT;
    bytes[3] = ABI_WASM_LIST_MAGIC;
    bytes[ABI_WASM_LIST_ELEMS + cap] = ABI_WASM_LIST_MAGIC;
    return (int64_t)(intptr_t)bytes;
}

static int64_t abi_bytes_from_raw(const unsigned char *src, int32_t len) {
    if (len < 0) len = 0;
    int64_t cap = len < 8 ? 8 : len;
    int64_t *bytes = (int64_t *)wasm_alloc((unsigned int)((ABI_WASM_LIST_ELEMS + cap + 1) * 8));
    if (!bytes) return abi_bytes_default();
    bytes[0] = cap;
    bytes[1] = len;
    bytes[2] = ABI_TAG_INT;
    bytes[3] = ABI_WASM_LIST_MAGIC;
    for (int32_t i = 0; i < len; i++) {
        bytes[ABI_WASM_LIST_ELEMS + i] = src ? (int64_t)src[i] : 0;
    }
    for (int64_t i = len; i < cap; i++) bytes[ABI_WASM_LIST_ELEMS + i] = 0;
    bytes[ABI_WASM_LIST_ELEMS + cap] = ABI_WASM_LIST_MAGIC;
    return (int64_t)(intptr_t)bytes;
}

static int abi_is_bytes_value(int64_t value) {
    if (!abi_wasm_is_readable(value, 16)) return 0;
    int64_t *bytes = (int64_t *)(intptr_t)value;
    if ((bytes[0] & 0xFFFFFFFFFFFFFF00LL) == ABI_BYTES_MAGIC) {
        int64_t len = bytes[1];
        if (len < 0 || len > TAIDA_ABI_WEB_MAX_REQUEST_BYTES) return 0;
        return abi_wasm_is_readable(value, (uint64_t)(2 + len) * 8u);
    }
    if (!abi_wasm_is_readable(value, ABI_WASM_LIST_ELEMS * 8u)) return 0;
    int64_t cap = bytes[0];
    int64_t len = bytes[1];
    if (cap < 8 || cap > TAIDA_ABI_WEB_MAX_REQUEST_BYTES) return 0;
    if (len < 0 || len > cap) return 0;
    uint64_t total_bytes = (uint64_t)(ABI_WASM_LIST_ELEMS + cap + 1) * 8u;
    if (!abi_wasm_is_readable(value, total_bytes)) return 0;
    return bytes[3] == ABI_WASM_LIST_MAGIC &&
        bytes[ABI_WASM_LIST_ELEMS + cap] == ABI_WASM_LIST_MAGIC;
}

static int32_t abi_bytes_value_len(int64_t value) {
    if (!abi_is_bytes_value(value)) return 0;
    int64_t len = ((int64_t *)(intptr_t)value)[1];
    if (len < 0) return 0;
    if (len > TAIDA_ABI_WEB_MAX_REQUEST_BYTES) return TAIDA_ABI_WEB_MAX_REQUEST_BYTES;
    return (int32_t)len;
}

static unsigned char abi_bytes_value_at(int64_t value, int32_t index) {
    int64_t *bytes = (int64_t *)(intptr_t)value;
    if ((bytes[0] & 0xFFFFFFFFFFFFFF00LL) == ABI_BYTES_MAGIC) {
        return (unsigned char)(bytes[2 + index] & 0xff);
    }
    return (unsigned char)(bytes[ABI_WASM_LIST_ELEMS + index] & 0xff);
}

static int64_t abi_body_to_bytes(int64_t body) {
    if (abi_is_bytes_value(body)) return body;
    const char *s = (const char *)(intptr_t)body;
    return abi_bytes_from_raw((const unsigned char *)(s ? s : ""), abi_strlen(s));
}

static int64_t abi_response_new(int64_t status, int64_t headers, int64_t body) {
    int64_t pack = taida_pack_new(3);
    taida_pack_set_hash(pack, 0, abi_hash_cstr("status"));
    taida_pack_set_tag(pack, 0, ABI_TAG_INT);
    taida_pack_set(pack, 0, abi_status_clamp(status));
    taida_pack_set_hash(pack, 1, abi_hash_cstr("headers"));
    taida_pack_set_tag(pack, 1, ABI_TAG_LIST);
    taida_pack_set(pack, 1, headers ? headers : abi_pair_list_new());
    taida_pack_set_hash(pack, 2, abi_hash_cstr("body"));
    taida_pack_set_tag(pack, 2, ABI_TAG_PACK);
    taida_pack_set(pack, 2, body ? abi_body_to_bytes(body) : abi_bytes_default());
    return pack;
}

static int64_t abi_error_response(int64_t status, const char *message) {
    int64_t headers = abi_pair_list_new();
    headers = abi_pair_list_append_raw(headers, "x-taida-error", "abi");
    int64_t body = abi_bytes_from_raw(
        (const unsigned char *)(message ? message : "handler error"),
        abi_strlen(message ? message : "handler error")
    );
    return abi_response_new(status, headers, body);
}

int64_t taida_abi_response_text(int64_t body_ptr) {
    const char *body = (const char *)(intptr_t)body_ptr;
    int64_t headers = abi_pair_list_new();
    headers = abi_header_list_append(headers, "content-type", "text/plain; charset=utf-8");
    return abi_response_new(
        200,
        headers,
        abi_bytes_from_raw((const unsigned char *)(body ? body : ""), abi_strlen(body))
    );
}

int64_t taida_abi_response_json(int64_t value) {
    int64_t encoded = taida_json_encode(value);
    const char *body = (const char *)(intptr_t)encoded;
    int64_t headers = abi_pair_list_new();
    headers = abi_header_list_append(headers, "content-type", "application/json");
    return abi_response_new(
        200,
        headers,
        abi_bytes_from_raw((const unsigned char *)(body ? body : ""), abi_strlen(body))
    );
}

int64_t taida_abi_response_bytes(int64_t body_ptr) {
    int64_t headers = abi_pair_list_new();
    headers = abi_header_list_append(headers, "content-type", "application/octet-stream");
    return abi_response_new(200, headers, abi_body_to_bytes(body_ptr));
}

int64_t taida_abi_response_status(int64_t code, int64_t response) {
    int64_t headers = taida_pack_get(response, abi_hash_cstr("headers"));
    int64_t body = taida_pack_get(response, abi_hash_cstr("body"));
    return abi_response_new(abi_status_clamp(code), headers, body);
}

int64_t taida_abi_response_header(int64_t name_ptr, int64_t value_ptr, int64_t response) {
    const char *name = (const char *)(intptr_t)name_ptr;
    const char *value = (const char *)(intptr_t)value_ptr;
    if (!abi_header_name_valid(name) || !abi_header_value_valid(value)) {
        return abi_error_response(500, "invalid response header");
    }
    int64_t headers = abi_pair_list_copy(taida_pack_get(response, abi_hash_cstr("headers")));
    headers = abi_header_list_append(
        headers,
        name,
        value
    );
    int64_t status = taida_pack_get(response, abi_hash_cstr("status"));
    if (status == 0) status = 200;
    int64_t body = taida_pack_get(response, abi_hash_cstr("body"));
    return abi_response_new(abi_status_clamp(status), headers, body);
}

int32_t taida_abi_web_alloc(int32_t len) {
    if (len < 0 || len > TAIDA_ABI_WEB_MAX_REQUEST_BYTES) return 0;
    int32_t arena_mark = wasm_arena_enter();
    char *buf = (char *)wasm_alloc((unsigned int)(len + 1));
    if (!buf) {
        wasm_arena_leave(arena_mark);
        return 0;
    }
    buf[len] = '\0';
    TaidaAbiWebAlloc *entry = &abi_web_allocs[abi_web_alloc_next];
    entry->active = 1;
    entry->ptr = (int32_t)(intptr_t)buf;
    entry->len = len;
    entry->arena_mark = arena_mark;
    abi_web_alloc_next = (abi_web_alloc_next + 1) % TAIDA_ABI_WEB_ALLOC_TABLE_SIZE;
    return (int32_t)(intptr_t)buf;
}

int32_t taida_abi_web_begin_request(int32_t ptr, int32_t len) {
    for (int32_t i = 0; i < TAIDA_ABI_WEB_ALLOC_TABLE_SIZE; i++) {
        TaidaAbiWebAlloc *entry = &abi_web_allocs[i];
        if (!entry->active) continue;
        if (entry->ptr == ptr && len >= 0 && len <= entry->len && entry->arena_mark > 0) {
            abi_web_current_arena_mark = entry->arena_mark;
            return entry->arena_mark;
        }
    }
    abi_web_current_arena_mark = wasm_arena_enter();
    return abi_web_current_arena_mark;
}

int32_t taida_abi_web_validate_request(int32_t ptr, int32_t len) {
    if (ptr <= 0 || len < 0 || len > TAIDA_ABI_WEB_MAX_REQUEST_BYTES) return 0;
    for (int32_t i = 0; i < TAIDA_ABI_WEB_ALLOC_TABLE_SIZE; i++) {
        TaidaAbiWebAlloc *entry = &abi_web_allocs[i];
        if (!entry->active) continue;
        if (entry->ptr == ptr && len <= entry->len) return 1;
    }
    return 0;
}

static void abi_json_skip_ws(const char *json, int32_t len, int32_t *p) {
    while (*p < len) {
        char c = json[*p];
        if (c != ' ' && c != '\n' && c != '\r' && c != '\t') return;
        (*p)++;
    }
}

static void abi_json_skip_string_raw(const char *json, int32_t len, int32_t *p) {
    if (*p >= len || json[*p] != '"') return;
    (*p)++;
    while (*p < len) {
        char c = json[*p];
        (*p)++;
        if (c == '\\' && *p < len) {
            (*p)++;
            continue;
        }
        if (c == '"') return;
    }
}

static void abi_json_skip_value_raw(const char *json, int32_t len, int32_t *p) {
    abi_json_skip_ws(json, len, p);
    if (*p >= len) return;
    if (json[*p] == '"') {
        abi_json_skip_string_raw(json, len, p);
        return;
    }
    if (json[*p] == '{' || json[*p] == '[') {
        char open = json[*p];
        char close = open == '{' ? '}' : ']';
        int32_t depth = 1;
        (*p)++;
        while (*p < len && depth > 0) {
            if (json[*p] == '"') {
                abi_json_skip_string_raw(json, len, p);
                continue;
            }
            if (json[*p] == open) depth++;
            if (json[*p] == close) depth--;
            (*p)++;
        }
        return;
    }
    while (*p < len && json[*p] != ',' && json[*p] != '}' && json[*p] != ']') {
        (*p)++;
    }
}

static int abi_json_find_field_raw(
    const char *json,
    int32_t len,
    const char *key,
    int32_t *value_start,
    int32_t *value_end
) {
    int32_t p = 0;
    abi_json_skip_ws(json, len, &p);
    if (p >= len || json[p] != '{') return 0;
    p++;
    while (p < len) {
        abi_json_skip_ws(json, len, &p);
        if (p >= len || json[p] == '}') return 0;
        char *field = abi_json_parse_string(json, len, &p);
        abi_json_skip_ws(json, len, &p);
        if (p >= len || json[p] != ':') return 0;
        p++;
        abi_json_skip_ws(json, len, &p);
        int32_t start = p;
        abi_json_skip_value_raw(json, len, &p);
        int32_t end = p;
        if (field && abi_cstr_eq(field, key)) {
            *value_start = start;
            *value_end = end;
            return 1;
        }
        abi_json_skip_ws(json, len, &p);
        if (p < len && json[p] == ',') p++;
    }
    return 0;
}

static int abi_json_field_bool(const char *json, int32_t len, const char *key, int *out) {
    int32_t start = 0;
    int32_t end = 0;
    if (!abi_json_find_field_raw(json, len, key, &start, &end)) return 0;
    if (end - start >= 4 && json[start] == 't' && json[start + 1] == 'r' &&
        json[start + 2] == 'u' && json[start + 3] == 'e') {
        *out = 1;
        return 1;
    }
    if (end - start >= 5 && json[start] == 'f' && json[start + 1] == 'a' &&
        json[start + 2] == 'l' && json[start + 3] == 's' && json[start + 4] == 'e') {
        *out = 0;
        return 1;
    }
    return 0;
}

static char *abi_json_field_string(const char *json, int32_t len, const char *key) {
    int32_t start = 0;
    int32_t end = 0;
    if (!abi_json_find_field_raw(json, len, key, &start, &end)) return (char *)0;
    int32_t p = start;
    return abi_json_parse_string(json, end, &p);
}

static char *abi_json_field_raw_copy(const char *json, int32_t len, const char *key) {
    int32_t start = 0;
    int32_t end = 0;
    if (!abi_json_find_field_raw(json, len, key, &start, &end)) return (char *)0;
    return abi_copy_bytes(json + start, end - start);
}

static int abi_hex_value(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return 10 + (c - 'a');
    if (c >= 'A' && c <= 'F') return 10 + (c - 'A');
    return -1;
}

static void abi_json_append_utf8(char *out, int32_t *out_len, int cp) {
    if (cp <= 0 || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) {
        out[(*out_len)++] = '?';
    } else if (cp <= 0x7f) {
        out[(*out_len)++] = (char)cp;
    } else if (cp <= 0x7ff) {
        out[(*out_len)++] = (char)(0xc0 | ((cp >> 6) & 0x1f));
        out[(*out_len)++] = (char)(0x80 | (cp & 0x3f));
    } else if (cp <= 0xffff) {
        out[(*out_len)++] = (char)(0xe0 | ((cp >> 12) & 0x0f));
        out[(*out_len)++] = (char)(0x80 | ((cp >> 6) & 0x3f));
        out[(*out_len)++] = (char)(0x80 | (cp & 0x3f));
    } else {
        out[(*out_len)++] = (char)(0xf0 | ((cp >> 18) & 0x07));
        out[(*out_len)++] = (char)(0x80 | ((cp >> 12) & 0x3f));
        out[(*out_len)++] = (char)(0x80 | ((cp >> 6) & 0x3f));
        out[(*out_len)++] = (char)(0x80 | (cp & 0x3f));
    }
}

static char *abi_json_parse_string(const char *json, int32_t len, int32_t *p) {
    if (*p >= len || json[*p] != '"') return (char *)0;
    (*p)++;
    char *out = (char *)wasm_alloc((unsigned int)(len - *p + 1));
    int32_t out_len = 0;
    while (*p < len) {
        char c = json[*p];
        (*p)++;
        if (c == '"') {
            out[out_len] = '\0';
            return out;
        }
        if (c == '\\' && *p < len) {
            char esc = json[*p];
            (*p)++;
            switch (esc) {
                case '"': out[out_len++] = '"'; break;
                case '\\': out[out_len++] = '\\'; break;
                case '/': out[out_len++] = '/'; break;
                case 'b': out[out_len++] = '\b'; break;
                case 'f': out[out_len++] = '\f'; break;
                case 'n': out[out_len++] = '\n'; break;
                case 'r': out[out_len++] = '\r'; break;
                case 't': out[out_len++] = '\t'; break;
                case 'u': {
                    if (*p + 4 <= len) {
                        int h0 = abi_hex_value(json[*p]);
                        int h1 = abi_hex_value(json[*p + 1]);
                        int h2 = abi_hex_value(json[*p + 2]);
                        int h3 = abi_hex_value(json[*p + 3]);
                        if (h0 >= 0 && h1 >= 0 && h2 >= 0 && h3 >= 0) {
                            int cp = (h0 << 12) | (h1 << 8) | (h2 << 4) | h3;
                            *p += 4;
                            if (cp >= 0xd800 && cp <= 0xdbff &&
                                *p + 6 <= len && json[*p] == '\\' && json[*p + 1] == 'u') {
                                int l0 = abi_hex_value(json[*p + 2]);
                                int l1 = abi_hex_value(json[*p + 3]);
                                int l2 = abi_hex_value(json[*p + 4]);
                                int l3 = abi_hex_value(json[*p + 5]);
                                if (l0 >= 0 && l1 >= 0 && l2 >= 0 && l3 >= 0) {
                                    int low = (l0 << 12) | (l1 << 8) | (l2 << 4) | l3;
                                    if (low >= 0xdc00 && low <= 0xdfff) {
                                        cp = 0x10000 + ((cp - 0xd800) << 10) + (low - 0xdc00);
                                        *p += 6;
                                    }
                                }
                            }
                            abi_json_append_utf8(out, &out_len, cp);
                        }
                    }
                    break;
                }
                default:
                    out[out_len++] = esc;
                    break;
            }
        } else {
            out[out_len++] = c;
        }
    }
    out[out_len] = '\0';
    return out;
}

static int abi_json_key_matches(const char *json, int32_t len, int32_t *p, const char *key) {
    int32_t start = *p;
    char *parsed = abi_json_parse_string(json, len, p);
    if (!parsed) {
        *p = start;
        return 0;
    }
    int32_t i = 0;
    while (key[i] && parsed[i] && key[i] == parsed[i]) i++;
    return key[i] == '\0' && parsed[i] == '\0';
}

static char *abi_json_find_string(const char *json, int32_t len, const char *key, const char *fallback) {
    for (int32_t p = 0; p < len; p++) {
        if (json[p] != '"') continue;
        int32_t cursor = p;
        if (!abi_json_key_matches(json, len, &cursor, key)) continue;
        abi_json_skip_ws(json, len, &cursor);
        if (cursor >= len || json[cursor] != ':') continue;
        cursor++;
        abi_json_skip_ws(json, len, &cursor);
        char *value = abi_json_parse_string(json, len, &cursor);
        if (value) return value;
    }
    return abi_copy_cstr(fallback);
}

static int abi_json_find_object(const char *json, int32_t len, const char *key, int32_t *start, int32_t *end) {
    for (int32_t p = 0; p < len; p++) {
        if (json[p] != '"') continue;
        int32_t cursor = p;
        if (!abi_json_key_matches(json, len, &cursor, key)) continue;
        abi_json_skip_ws(json, len, &cursor);
        if (cursor >= len || json[cursor] != ':') continue;
        cursor++;
        abi_json_skip_ws(json, len, &cursor);
        if (cursor >= len || json[cursor] != '{') continue;
        cursor++;
        *start = cursor;
        int depth = 1;
        int in_str = 0;
        int esc = 0;
        while (cursor < len) {
            char c = json[cursor++];
            if (in_str) {
                if (esc) {
                    esc = 0;
                } else if (c == '\\') {
                    esc = 1;
                } else if (c == '"') {
                    in_str = 0;
                }
                continue;
            }
            if (c == '"') {
                in_str = 1;
            } else if (c == '{') {
                depth++;
            } else if (c == '}') {
                depth--;
                if (depth == 0) {
                    *end = cursor - 1;
                    return 1;
                }
            }
        }
    }
    *start = 0;
    *end = 0;
    return 0;
}

static int abi_json_find_array(const char *json, int32_t len, const char *key, int32_t *start, int32_t *end) {
    for (int32_t p = 0; p < len; p++) {
        if (json[p] != '"') continue;
        int32_t cursor = p;
        if (!abi_json_key_matches(json, len, &cursor, key)) continue;
        abi_json_skip_ws(json, len, &cursor);
        if (cursor >= len || json[cursor] != ':') continue;
        cursor++;
        abi_json_skip_ws(json, len, &cursor);
        if (cursor >= len || json[cursor] != '[') continue;
        cursor++;
        *start = cursor;
        int depth = 1;
        int in_str = 0;
        int esc = 0;
        while (cursor < len) {
            char c = json[cursor++];
            if (in_str) {
                if (esc) {
                    esc = 0;
                } else if (c == '\\') {
                    esc = 1;
                } else if (c == '"') {
                    in_str = 0;
                }
                continue;
            }
            if (c == '"') {
                in_str = 1;
            } else if (c == '[') {
                depth++;
            } else if (c == ']') {
                depth--;
                if (depth == 0) {
                    *end = cursor - 1;
                    return 1;
                }
            }
        }
    }
    *start = 0;
    *end = 0;
    return 0;
}

static char *abi_json_find_string_in_range(
    const char *json,
    int32_t start,
    int32_t end,
    const char *key
) {
    for (int32_t p = start; p < end; p++) {
        if (json[p] != '"') continue;
        int32_t cursor = p;
        if (!abi_json_key_matches(json, end, &cursor, key)) continue;
        abi_json_skip_ws(json, end, &cursor);
        if (cursor >= end || json[cursor] != ':') continue;
        cursor++;
        abi_json_skip_ws(json, end, &cursor);
        char *value = abi_json_parse_string(json, end, &cursor);
        if (value) return value;
    }
    return (char *)0;
}

static int64_t abi_json_pair_list(const char *json, int32_t len, const char *key, int validate_headers) {
    int64_t list = abi_pair_list_new();
    int32_t start = 0;
    int32_t end = 0;
    if (!abi_json_find_array(json, len, key, &start, &end)) return list;
    int32_t p = start;
    int32_t count = 0;
    while (p < end) {
        if (count >= TAIDA_ABI_WEB_MAX_HEADERS) break;
        abi_json_skip_ws(json, end, &p);
        if (p >= end) break;
        if (json[p] == ',') {
            p++;
            continue;
        }
        if (json[p] != '{') break;
        p++;
        int32_t obj_start = p;
        int depth = 1;
        int in_str = 0;
        int esc = 0;
        while (p < end) {
            char c = json[p++];
            if (in_str) {
                if (esc) {
                    esc = 0;
                } else if (c == '\\') {
                    esc = 1;
                } else if (c == '"') {
                    in_str = 0;
                }
                continue;
            }
            if (c == '"') {
                in_str = 1;
            } else if (c == '{') {
                depth++;
            } else if (c == '}') {
                depth--;
                if (depth == 0) break;
            }
        }
        if (depth != 0) break;
        int32_t obj_end = p - 1;
        char *name = abi_json_find_string_in_range(json, obj_start, obj_end, "name");
        char *value = abi_json_find_string_in_range(json, obj_start, obj_end, "value");
        if (name && value) {
            list = validate_headers
                ? abi_header_list_append(list, name, value)
                : abi_pair_list_append_raw(list, name, value);
            count++;
        }
        abi_json_skip_ws(json, end, &p);
        if (p < end && json[p] == ',') p++;
    }
    return list;
}

static int64_t abi_json_legacy_string_map_as_pair_list(const char *json, int32_t len, const char *key, int validate_headers) {
    int64_t list = abi_pair_list_new();
    int32_t start = 0;
    int32_t end = 0;
    if (!abi_json_find_object(json, len, key, &start, &end)) return list;
    int32_t p = start;
    int32_t count = 0;
    while (p < end) {
        if (count >= TAIDA_ABI_WEB_MAX_HEADERS) break;
        abi_json_skip_ws(json, end, &p);
        if (p >= end) break;
        if (json[p] == ',') {
            p++;
            continue;
        }
        char *map_key = abi_json_parse_string(json, end, &p);
        if (!map_key) break;
        abi_json_skip_ws(json, end, &p);
        if (p >= end || json[p] != ':') break;
        p++;
        abi_json_skip_ws(json, end, &p);
        char *map_value = abi_json_parse_string(json, end, &p);
        if (!map_value) break;
        list = validate_headers
            ? abi_header_list_append(list, map_key, map_value)
            : abi_pair_list_append_raw(list, map_key, map_value);
        count++;
        abi_json_skip_ws(json, end, &p);
        if (p < end && json[p] == ',') p++;
    }
    return list;
}

static int64_t abi_json_request_pairs(const char *json, int32_t len, const char *key, int validate_headers) {
    int32_t start = 0;
    int32_t end = 0;
    if (abi_json_find_array(json, len, key, &start, &end)) {
        return abi_json_pair_list(json, len, key, validate_headers);
    }
    return abi_json_legacy_string_map_as_pair_list(json, len, key, validate_headers);
}

static int abi_b64_value(char c) {
    if (c >= 'A' && c <= 'Z') return c - 'A';
    if (c >= 'a' && c <= 'z') return 26 + (c - 'a');
    if (c >= '0' && c <= '9') return 52 + (c - '0');
    if (c == '+') return 62;
    if (c == '/') return 63;
    return -1;
}

static char *abi_base64_decode(const char *src, int32_t *out_len) {
    int32_t len = abi_strlen(src);
    if (len > TAIDA_ABI_WEB_MAX_REQUEST_BYTES) {
        if (out_len) *out_len = 0;
        return abi_copy_cstr("");
    }
    char *out = (char *)wasm_alloc((unsigned int)((len / 4) * 3 + 4));
    if (!out) {
        if (out_len) *out_len = 0;
        return abi_copy_cstr("");
    }
    int32_t opos = 0;
    int buf = 0;
    int bits = 0;
    for (int32_t i = 0; i < len; i++) {
        char c = src[i];
        if (c == '=') break;
        if (c == ' ' || c == '\n' || c == '\r' || c == '\t') continue;
        int v = abi_b64_value(c);
        if (v < 0) {
            opos = 0;
            break;
        }
        buf = (buf << 6) | v;
        bits += 6;
        if (bits >= 8) {
            bits -= 8;
            out[opos++] = (char)((buf >> bits) & 0xff);
        }
    }
    out[opos] = '\0';
    if (out_len) *out_len = opos;
    return out;
}

int64_t taida_abi_web_make_request(int32_t ptr, int32_t len) {
    const char *json = (const char *)(intptr_t)ptr;
    if (!json || len < 0 || !taida_abi_web_validate_request(ptr, len)) {
        json = "{}";
        len = 2;
    }
    char *method = abi_json_find_string(json, len, "method", "GET");
    char *path = abi_json_find_string(json, len, "path", "/");
    char *raw_query = abi_json_find_string(json, len, "rawQuery", "");
    int64_t query = abi_json_request_pairs(json, len, "query", 0);
    int64_t headers = abi_json_request_pairs(json, len, "headers", 1);
    char *body_b64 = abi_json_find_string(json, len, "bodyBase64", "");
    int32_t body_len = 0;
    char *body = abi_base64_decode(body_b64, &body_len);
    int64_t body_bytes = abi_bytes_from_raw((const unsigned char *)body, body_len);

    int64_t pack = taida_pack_new(6);
    taida_pack_set_hash(pack, 0, abi_hash_cstr("method"));
    taida_pack_set_tag(pack, 0, ABI_TAG_STR);
    taida_pack_set(pack, 0, (int64_t)(intptr_t)method);
    taida_pack_set_hash(pack, 1, abi_hash_cstr("path"));
    taida_pack_set_tag(pack, 1, ABI_TAG_STR);
    taida_pack_set(pack, 1, (int64_t)(intptr_t)path);
    taida_pack_set_hash(pack, 2, abi_hash_cstr("rawQuery"));
    taida_pack_set_tag(pack, 2, ABI_TAG_STR);
    taida_pack_set(pack, 2, (int64_t)(intptr_t)raw_query);
    taida_pack_set_hash(pack, 3, abi_hash_cstr("query"));
    taida_pack_set_tag(pack, 3, ABI_TAG_LIST);
    taida_pack_set(pack, 3, query);
    taida_pack_set_hash(pack, 4, abi_hash_cstr("headers"));
    taida_pack_set_tag(pack, 4, ABI_TAG_LIST);
    taida_pack_set(pack, 4, headers);
    taida_pack_set_hash(pack, 5, abi_hash_cstr("body"));
    taida_pack_set_tag(pack, 5, ABI_TAG_PACK);
    taida_pack_set(pack, 5, body_bytes);
    return pack;
}

static void abi_jb_init(TaidaAbiJsonBuilder *jb, int32_t cap) {
    jb->cap = cap < 64 ? 64 : cap;
    jb->len = 0;
    jb->buf = (char *)wasm_alloc((unsigned int)jb->cap);
    jb->buf[0] = '\0';
}

static void abi_jb_reserve(TaidaAbiJsonBuilder *jb, int32_t extra) {
    if (jb->len + extra + 1 <= jb->cap) return;
    int32_t new_cap = jb->cap * 2;
    while (jb->len + extra + 1 > new_cap) new_cap *= 2;
    char *next = (char *)wasm_alloc((unsigned int)new_cap);
    abi_memcpy(next, jb->buf, jb->len);
    next[jb->len] = '\0';
    jb->buf = next;
    jb->cap = new_cap;
}

static void abi_jb_append_len(TaidaAbiJsonBuilder *jb, const char *s, int32_t len) {
    abi_jb_reserve(jb, len);
    abi_memcpy(jb->buf + jb->len, s, len);
    jb->len += len;
    jb->buf[jb->len] = '\0';
}

static void abi_jb_append(TaidaAbiJsonBuilder *jb, const char *s) {
    abi_jb_append_len(jb, s, abi_strlen(s));
}

static void abi_jb_append_int(TaidaAbiJsonBuilder *jb, int64_t n) {
    char tmp[32];
    int32_t pos = 31;
    int neg = n < 0;
    uint64_t v = neg ? (uint64_t)(-n) : (uint64_t)n;
    tmp[pos--] = '\0';
    do {
        tmp[pos--] = (char)('0' + (v % 10));
        v /= 10;
    } while (v);
    if (neg) tmp[pos--] = '-';
    abi_jb_append(jb, &tmp[pos + 1]);
}

static char abi_hex_digit(int n) {
    return (char)(n < 10 ? ('0' + n) : ('a' + (n - 10)));
}

static void abi_jb_append_json_string(TaidaAbiJsonBuilder *jb, const char *s) {
    abi_jb_append(jb, "\"");
    for (int32_t i = 0; s && s[i]; i++) {
        unsigned char c = (unsigned char)s[i];
        if (c == '"' || c == '\\') {
            abi_jb_append_len(jb, "\\", 1);
            char ch = (char)c;
            abi_jb_append_len(jb, &ch, 1);
        } else if (c == '\n') {
            abi_jb_append(jb, "\\n");
        } else if (c == '\r') {
            abi_jb_append(jb, "\\r");
        } else if (c == '\t') {
            abi_jb_append(jb, "\\t");
        } else if (c < 0x20) {
            char esc[6];
            esc[0] = '\\';
            esc[1] = 'u';
            esc[2] = '0';
            esc[3] = '0';
            esc[4] = abi_hex_digit((c >> 4) & 0x0f);
            esc[5] = abi_hex_digit(c & 0x0f);
            abi_jb_append_len(jb, esc, 6);
        } else {
            char ch = (char)c;
            abi_jb_append_len(jb, &ch, 1);
        }
    }
    abi_jb_append(jb, "\"");
}

static char abi_b64_char(int n) {
    static const char table[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    return table[n & 63];
}

static void abi_jb_append_base64(TaidaAbiJsonBuilder *jb, const unsigned char *data, int32_t len) {
    for (int32_t i = 0; i < len; i += 3) {
        int rem = len - i;
        unsigned int b0 = data[i];
        unsigned int b1 = rem > 1 ? data[i + 1] : 0;
        unsigned int b2 = rem > 2 ? data[i + 2] : 0;
        char out[4];
        out[0] = abi_b64_char((b0 >> 2) & 63);
        out[1] = abi_b64_char(((b0 & 3) << 4) | ((b1 >> 4) & 15));
        out[2] = rem > 1 ? abi_b64_char(((b1 & 15) << 2) | ((b2 >> 6) & 3)) : '=';
        out[3] = rem > 2 ? abi_b64_char(b2 & 63) : '=';
        abi_jb_append_len(jb, out, 4);
    }
}

static void abi_jb_append_base64_bytes(TaidaAbiJsonBuilder *jb, int64_t bytes_value) {
    int32_t len = abi_bytes_value_len(bytes_value);
    for (int32_t i = 0; i < len; i += 3) {
        int rem = len - i;
        unsigned int b0 = abi_bytes_value_at(bytes_value, i);
        unsigned int b1 = rem > 1 ? abi_bytes_value_at(bytes_value, i + 1) : 0;
        unsigned int b2 = rem > 2 ? abi_bytes_value_at(bytes_value, i + 2) : 0;
        char out[4];
        out[0] = abi_b64_char((b0 >> 2) & 63);
        out[1] = abi_b64_char(((b0 & 3) << 4) | ((b1 >> 4) & 15));
        out[2] = rem > 1 ? abi_b64_char(((b1 & 15) << 2) | ((b2 >> 6) & 3)) : '=';
        out[3] = rem > 2 ? abi_b64_char(b2 & 63) : '=';
        abi_jb_append_len(jb, out, 4);
    }
}

static void abi_jb_append_headers(TaidaAbiJsonBuilder *jb, int64_t headers) {
    abi_jb_append(jb, "[");
    if (headers && abi_wasm_is_readable(headers, ABI_WASM_LIST_ELEMS * 8u)) {
        int64_t *list = (int64_t *)(intptr_t)headers;
        int64_t len = list[1];
        if (len < 0 || len > TAIDA_ABI_WEB_MAX_HEADERS) len = 0;
        int first = 1;
        for (int64_t i = 0; i < len; i++) {
            int64_t pair = list[ABI_WASM_LIST_ELEMS + i];
            int64_t sk = taida_pack_get(pair, abi_hash_cstr("name"));
            int64_t sv = taida_pack_get(pair, abi_hash_cstr("value"));
            const char *name = (const char *)(intptr_t)sk;
            const char *value = (const char *)(intptr_t)sv;
            if (!abi_header_name_valid(name) || !abi_header_value_valid(value)) continue;
            if (!first) abi_jb_append(jb, ",");
            first = 0;
            abi_jb_append(jb, "{\"name\":");
            abi_jb_append_json_string(jb, name);
            abi_jb_append(jb, ",\"value\":");
            abi_jb_append_json_string(jb, value);
            abi_jb_append(jb, "}");
        }
    }
    abi_jb_append(jb, "]");
}

static int abi_is_list_value(int64_t value) {
    if (!abi_wasm_is_readable(value, ABI_WASM_LIST_ELEMS * 8u)) return 0;
    int64_t *list = (int64_t *)(intptr_t)value;
    int64_t cap = list[0];
    int64_t len = list[1];
    if (cap < 0 || cap > TAIDA_ABI_WEB_MAX_REQUEST_BYTES) return 0;
    if (len < 0 || len > cap) return 0;
    uint64_t total_bytes = (uint64_t)(ABI_WASM_LIST_ELEMS + cap + 1) * 8u;
    if (!abi_wasm_is_readable(value, total_bytes)) return 0;
    return list[3] == ABI_WASM_LIST_MAGIC &&
        list[ABI_WASM_LIST_ELEMS + cap] == ABI_WASM_LIST_MAGIC;
}

static int64_t abi_host_error(const char *message) {
    return taida_make_error(
        (int64_t)(intptr_t)"HostCapabilityError",
        (int64_t)(intptr_t)(message ? message : "host capability error"));
}

int64_t taida_abi_host_capability(int64_t name, int64_t kind) {
    int64_t pack = taida_pack_new(3);
    taida_pack_set_hash(pack, 0, abi_hash_cstr("__type"));
    taida_pack_set_tag(pack, 0, ABI_TAG_STR);
    taida_pack_set(pack, 0, (int64_t)(intptr_t)"HostCapability");
    taida_pack_set_hash(pack, 1, abi_hash_cstr("name"));
    taida_pack_set_tag(pack, 1, ABI_TAG_STR);
    taida_pack_set(pack, 1, name);
    taida_pack_set_hash(pack, 2, abi_hash_cstr("kind"));
    taida_pack_set_tag(pack, 2, ABI_TAG_STR);
    taida_pack_set(pack, 2, kind);
    return pack;
}

int64_t taida_abi_host_step(int64_t method, int64_t args, int64_t args_schema) {
    int64_t pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, abi_hash_cstr("__type"));
    taida_pack_set_tag(pack, 0, ABI_TAG_STR);
    taida_pack_set(pack, 0, (int64_t)(intptr_t)"HostStep");
    taida_pack_set_hash(pack, 1, abi_hash_cstr("method"));
    taida_pack_set_tag(pack, 1, ABI_TAG_STR);
    taida_pack_set(pack, 1, method);
    taida_pack_set_hash(pack, 2, abi_hash_cstr("args"));
    taida_pack_set_tag(pack, 2, ABI_TAG_LIST);
    taida_pack_set(pack, 2, args);
    taida_pack_set_hash(pack, 3, abi_hash_cstr("args_schema"));
    taida_pack_set_tag(pack, 3, ABI_TAG_STR);
    taida_pack_set(pack, 3, args_schema);
    return pack;
}

int64_t taida_abi_host_call(int64_t steps, int64_t schema) {
    int64_t pack = taida_pack_new(3);
    taida_pack_set_hash(pack, 0, abi_hash_cstr("__type"));
    taida_pack_set_tag(pack, 0, ABI_TAG_STR);
    taida_pack_set(pack, 0, (int64_t)(intptr_t)"HostCall");
    taida_pack_set_hash(pack, 1, abi_hash_cstr("steps"));
    taida_pack_set_tag(pack, 1, ABI_TAG_LIST);
    taida_pack_set(pack, 1, steps);
    taida_pack_set_hash(pack, 2, abi_hash_cstr("schema"));
    taida_pack_set_tag(pack, 2, ABI_TAG_STR);
    taida_pack_set(pack, 2, schema);
    return pack;
}

static void abi_jb_append_host_steps(TaidaAbiJsonBuilder *jb, int64_t steps) {
    abi_jb_append(jb, "[");
    if (abi_is_list_value(steps)) {
        int64_t *list = (int64_t *)(intptr_t)steps;
        int64_t len = list[1];
        int first = 1;
        for (int64_t i = 0; i < len; i++) {
            int64_t step = list[ABI_WASM_LIST_ELEMS + i];
            int64_t method = taida_pack_get(step, abi_hash_cstr("method"));
            int64_t args = taida_pack_get(step, abi_hash_cstr("args"));
            int64_t args_schema = taida_pack_get(step, abi_hash_cstr("args_schema"));
            const char *method_str = (const char *)(intptr_t)method;
            int64_t args_json = taida_json_encode_wire(args, args_schema);
            if (!first) abi_jb_append(jb, ",");
            first = 0;
            abi_jb_append(jb, "{\"method\":");
            abi_jb_append_json_string(jb, method_str ? method_str : "");
            abi_jb_append(jb, ",\"args\":");
            abi_jb_append(jb, (const char *)(intptr_t)args_json);
            abi_jb_append(jb, "}");
        }
    }
    abi_jb_append(jb, "]");
}

static int64_t abi_host_pending_error_value(void) {
    return (int64_t)(intptr_t)&abi_host_pending_marker;
}

int32_t taida_abi_web_is_host_call_pending_error(int64_t error) {
    return error == abi_host_pending_error_value() && abi_host_pending_json != (char *)0;
}

static void abi_host_set_pending_json(int64_t capability, int64_t call) {
    int64_t id = abi_host_next_id++;
    const char *capability_name =
        (const char *)(intptr_t)taida_pack_get(capability, abi_hash_cstr("name"));
    int64_t steps = taida_pack_get(call, abi_hash_cstr("steps"));
    TaidaAbiJsonBuilder jb;
    abi_jb_init(&jb, 256);
    abi_jb_append(&jb, "{\"id\":");
    abi_jb_append_int(&jb, id);
    abi_jb_append(&jb, ",\"kind\":\"host_call\",\"capability\":");
    abi_jb_append_json_string(&jb, capability_name ? capability_name : "");
    abi_jb_append(&jb, ",\"steps\":");
    abi_jb_append_host_steps(&jb, steps);
    abi_jb_append(&jb, "}");
    abi_host_pending_json = jb.buf;
    abi_host_pending_len = jb.len;
}

static int64_t abi_host_resume_error_async(const char *message) {
    return taida_async_err(abi_host_error(message ? message : "host call failed"));
}

int64_t taida_abi_host_cage(int64_t capability, int64_t call) {
    if (abi_host_resume_active) {
        const char *json = abi_host_resume_json ? abi_host_resume_json : "";
        int32_t len = abi_host_resume_len;
        int ok = 0;
        if (!abi_json_field_bool(json, len, "ok", &ok)) {
            abi_host_resume_active = 0;
            return abi_host_resume_error_async("host call resume missing ok");
        }
        if (!ok) {
            char *message = abi_json_field_string(json, len, "error");
            abi_host_resume_active = 0;
            return abi_host_resume_error_async(message ? message : "host call failed");
        }

        char *raw_value = abi_json_field_raw_copy(json, len, "value");
        if (!raw_value) raw_value = abi_copy_cstr("null");
        int64_t schema = taida_pack_get(call, abi_hash_cstr("schema"));
        int64_t lax = taida_json_schema_cast((int64_t)(intptr_t)raw_value, schema);
        int64_t has_value = taida_pack_get(lax, abi_hash_cstr("has_value"));
        if (has_value) {
            int64_t value = taida_pack_get(lax, abi_hash_cstr("__value"));
            abi_host_resume_active = 0;
            return taida_async_ok(value);
        }
        int64_t error = taida_pack_get(lax, abi_hash_cstr("__error"));
        if (!error) error = abi_host_error("host call result decode failed");
        abi_host_resume_active = 0;
        return taida_async_err(error);
    }

    abi_host_set_pending_json(capability, call);
    return taida_async_pending_with_error(abi_host_pending_error_value());
}

int64_t taida_abi_web_store_pending_host_call_json(int32_t request_ptr, int32_t request_len) {
    if (!abi_host_pending_json) {
        return taida_abi_web_store_error_response_json(
            500,
            (int64_t)(intptr_t)"host call payload missing");
    }
    for (int32_t probe = 0; probe < TAIDA_ABI_WEB_OUT_TABLE_SIZE; probe++) {
        int32_t slot = (abi_web_out_next + probe) % TAIDA_ABI_WEB_OUT_TABLE_SIZE;
        if (!abi_web_outs[slot].active) {
            TaidaAbiWebOut *out = &abi_web_outs[slot];
            if (out->generation == 0) out->generation = 1;
            out->active = 1;
            out->state = 1;
            out->ptr = (int32_t)(intptr_t)abi_host_pending_json;
            out->len = abi_host_pending_len;
            out->arena_mark = abi_web_current_arena_mark;
            out->request_ptr = request_ptr;
            out->request_len = request_len;
            abi_host_pending_json = (char *)0;
            abi_host_pending_len = 0;
            abi_web_current_arena_mark = 0;
            abi_web_out_next = (slot + 1) % TAIDA_ABI_WEB_OUT_TABLE_SIZE;
            return ((int64_t)out->generation << 16) | (int64_t)(slot + 1);
        }
    }
    return taida_abi_web_store_error_response_json(
        500,
        (int64_t)(intptr_t)"host call output table full");
}

int64_t taida_abi_web_store_response_json(int64_t response) {
    int64_t status = taida_pack_get(response, abi_hash_cstr("status"));
    if (status == 0) status = 200;
    status = abi_status_clamp(status);
    int64_t headers = taida_pack_get(response, abi_hash_cstr("headers"));
    int64_t body_value = taida_pack_get(response, abi_hash_cstr("body"));
    int64_t body_bytes = abi_body_to_bytes(body_value);
    int32_t body_len = abi_bytes_value_len(body_bytes);

    TaidaAbiJsonBuilder jb;
    abi_jb_init(&jb, 256 + body_len * 2);
    abi_jb_append(&jb, "{\"status\":");
    abi_jb_append_int(&jb, status);
    abi_jb_append(&jb, ",\"headers\":");
    abi_jb_append_headers(&jb, headers);
    abi_jb_append(&jb, ",\"bodyBase64\":\"");
    abi_jb_append_base64_bytes(&jb, body_bytes);
    abi_jb_append(&jb, "\"}");

    for (int32_t probe = 0; probe < TAIDA_ABI_WEB_OUT_TABLE_SIZE; probe++) {
        int32_t slot = (abi_web_out_next + probe) % TAIDA_ABI_WEB_OUT_TABLE_SIZE;
        if (!abi_web_outs[slot].active) {
            TaidaAbiWebOut *out = &abi_web_outs[slot];
            if (out->generation == 0) out->generation = 1;
            out->active = 1;
            out->state = 0;
            out->ptr = (int32_t)(intptr_t)jb.buf;
            out->len = jb.len;
            out->arena_mark = abi_web_current_arena_mark;
            out->request_ptr = 0;
            out->request_len = 0;
            abi_web_current_arena_mark = 0;
            abi_web_out_next = (slot + 1) % TAIDA_ABI_WEB_OUT_TABLE_SIZE;
            return ((int64_t)out->generation << 16) | (int64_t)(slot + 1);
        }
    }
    TaidaAbiWebOut *out = &abi_web_outs[0];
    out->generation++;
    if (out->generation == 0) out->generation = 1;
    out->active = 1;
    out->state = 0;
    out->ptr = (int32_t)(intptr_t)jb.buf;
    out->len = jb.len;
    out->arena_mark = abi_web_current_arena_mark;
    out->request_ptr = 0;
    out->request_len = 0;
    abi_web_current_arena_mark = 0;
    abi_web_out_next = 1;
    return ((int64_t)out->generation << 16) | 1;
}

int64_t taida_abi_web_store_error_response_json(int64_t status, int64_t message_ptr) {
    const char *message = (const char *)(intptr_t)message_ptr;
    return taida_abi_web_store_response_json(abi_error_response(status, message));
}

static TaidaAbiWebOut *abi_web_out_get(int64_t handle) {
    if (handle <= 0) return (TaidaAbiWebOut *)0;
    uint64_t raw = (uint64_t)handle;
    int32_t slot = (int32_t)(raw & 0xffffu) - 1;
    uint32_t generation = (uint32_t)(raw >> 16);
    if (slot < 0 || slot >= TAIDA_ABI_WEB_OUT_TABLE_SIZE || generation == 0) {
        return (TaidaAbiWebOut *)0;
    }
    TaidaAbiWebOut *out = &abi_web_outs[slot];
    return (out->active && out->generation == generation) ? out : (TaidaAbiWebOut *)0;
}

int32_t taida_abi_web_poll(int64_t handle) {
    TaidaAbiWebOut *out = abi_web_out_get(handle);
    if (!out) return 2;
    if (out->state == 2) return 2;
    return out->state == 1 ? 1 : 0;
}

int32_t taida_abi_web_resume_begin(int64_t handle, int32_t ptr, int32_t len) {
    TaidaAbiWebOut *out = abi_web_out_get(handle);
    if (!out || out->state != 1) return 0;
    if (!taida_abi_web_validate_request(ptr, len)) return 0;
    abi_host_resume_json = (char *)(intptr_t)ptr;
    abi_host_resume_len = len;
    abi_host_resume_active = 1;
    return 1;
}

int32_t taida_abi_web_resume_request_ptr(int64_t handle) {
    TaidaAbiWebOut *out = abi_web_out_get(handle);
    return out ? out->request_ptr : 0;
}

int32_t taida_abi_web_resume_request_len(int64_t handle) {
    TaidaAbiWebOut *out = abi_web_out_get(handle);
    return out ? out->request_len : 0;
}

void taida_abi_web_replace_handle(int64_t dst_handle, int64_t src_handle) {
    TaidaAbiWebOut *dst = abi_web_out_get(dst_handle);
    TaidaAbiWebOut *src = abi_web_out_get(src_handle);
    if (!dst || !src) {
        if (dst) dst->state = 2;
        abi_host_resume_active = 0;
        return;
    }
    dst->state = src->state;
    dst->ptr = src->ptr;
    dst->len = src->len;
    dst->arena_mark = src->arena_mark;
    dst->request_ptr = src->request_ptr;
    dst->request_len = src->request_len;
    src->active = 0;
    src->ptr = 0;
    src->len = 0;
    src->arena_mark = 0;
    src->request_ptr = 0;
    src->request_len = 0;
    src->generation++;
    if (src->generation == 0) src->generation = 1;
    abi_host_resume_active = 0;
}

int32_t taida_abi_web_out_ptr(int64_t handle) {
    TaidaAbiWebOut *out = abi_web_out_get(handle);
    return out ? out->ptr : 0;
}

int32_t taida_abi_web_out_len(int64_t handle) {
    TaidaAbiWebOut *out = abi_web_out_get(handle);
    return out ? out->len : 0;
}

int32_t taida_abi_web_free(int64_t handle) {
    TaidaAbiWebOut *out = abi_web_out_get(handle);
    if (!out) return 0;
    int32_t arena_mark = out->arena_mark;
    out->active = 0;
    out->state = 0;
    out->ptr = 0;
    out->len = 0;
    out->arena_mark = 0;
    out->request_ptr = 0;
    out->request_len = 0;
    out->generation++;
    if (out->generation == 0) out->generation = 1;
    if (arena_mark > 0) {
        for (int32_t i = 0; i < TAIDA_ABI_WEB_ALLOC_TABLE_SIZE; i++) {
            TaidaAbiWebAlloc *entry = &abi_web_allocs[i];
            if (entry->active && entry->arena_mark >= arena_mark) {
                entry->active = 0;
                entry->ptr = 0;
                entry->len = 0;
                entry->arena_mark = 0;
            }
        }
        wasm_arena_leave(arena_mark);
    }
    return 1;
}
