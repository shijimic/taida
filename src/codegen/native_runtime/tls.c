// ── NET5-4a: OpenSSL dlopen TLS support ─────────────────────────────
// Load libssl/libcrypto at runtime via dlopen — no compile-time headers needed.
// This avoids requiring libssl-dev at build time while still providing
// TLS server capability when OpenSSL shared libraries are installed.
//
// Opaque handle types — we only ever pass pointers through.
typedef void OSSL_SSL_CTX;
typedef void OSSL_SSL;
typedef void OSSL_SSL_METHOD;
typedef void OSSL_BIO;
typedef void OSSL_X509;
typedef void OSSL_EVP_PKEY;

// Function pointer table for the OpenSSL symbols we need.
static struct {
    int loaded;
    void *libssl_handle;
    void *libcrypto_handle;
    // libssl functions
    OSSL_SSL_METHOD *(*TLS_server_method)(void);
    OSSL_SSL_CTX *(*SSL_CTX_new)(const OSSL_SSL_METHOD *method);
    void (*SSL_CTX_free)(OSSL_SSL_CTX *ctx);
    int (*SSL_CTX_use_certificate_chain_file)(OSSL_SSL_CTX *ctx, const char *file);
    int (*SSL_CTX_use_PrivateKey_file)(OSSL_SSL_CTX *ctx, const char *file, int type);
    int (*SSL_CTX_check_private_key)(const OSSL_SSL_CTX *ctx);
    OSSL_SSL *(*SSL_new)(OSSL_SSL_CTX *ctx);
    void (*SSL_free)(OSSL_SSL *ssl);
    int (*SSL_set_fd)(OSSL_SSL *ssl, int fd);
    int (*SSL_accept)(OSSL_SSL *ssl);
    int (*SSL_read)(OSSL_SSL *ssl, void *buf, int num);
    int (*SSL_write)(OSSL_SSL *ssl, const void *buf, int num);
    int (*SSL_shutdown)(OSSL_SSL *ssl);
    int (*SSL_get_error)(const OSSL_SSL *ssl, int ret);
    long (*SSL_CTX_set_options)(OSSL_SSL_CTX *ctx, long options);
    // ALPN server-side: negotiate h2 / http/1.1 for HTTP/2 support.
    // SSL_CTX_set_alpn_select_cb: server-side protocol selection callback.
    // SSL_select_next_proto: helper to pick from client's advertised list.
    // SSL_get0_alpn_selected: query the negotiated protocol after handshake.
    void (*SSL_CTX_set_alpn_select_cb)(OSSL_SSL_CTX *ctx,
        int (*cb)(OSSL_SSL *ssl, const unsigned char **out, unsigned char *outlen,
                  const unsigned char *in, unsigned int inlen, void *arg),
        void *arg);
    int (*SSL_select_next_proto)(unsigned char **out, unsigned char *outlen,
        const unsigned char *server, unsigned int server_len,
        const unsigned char *client, unsigned int client_len);
    void (*SSL_get0_alpn_selected)(const OSSL_SSL *ssl, const unsigned char **data, unsigned int *len);
} taida_ossl = {0};

// OpenSSL constants (stable ABI, unlikely to change).
#define TAIDA_SSL_FILETYPE_PEM 1
#define TAIDA_SSL_ERROR_NONE           0
#define TAIDA_SSL_ERROR_SSL            1
#define TAIDA_SSL_ERROR_WANT_READ      2
#define TAIDA_SSL_ERROR_WANT_WRITE     3
#define TAIDA_SSL_ERROR_SYSCALL        5
#define TAIDA_SSL_ERROR_ZERO_RETURN    6
// SSL_OP_NO_SSLv2 | SSL_OP_NO_SSLv3 | SSL_OP_NO_TLSv1 | SSL_OP_NO_TLSv1_1
// Only allow TLS 1.2+ for security.
#define TAIDA_SSL_OP_SECURE  (0x01000000L | 0x02000000L | 0x04000000L | 0x10000000L)

// Forward declaration.
static void taida_ossl_unload(void);

// Load OpenSSL shared libraries via dlopen. Returns 1 on success, 0 on failure.
static int taida_ossl_load(void) {
    if (taida_ossl.loaded) return 1;

    // Try common shared library names.
    taida_ossl.libssl_handle = dlopen("libssl.so.3", RTLD_LAZY);
    if (!taida_ossl.libssl_handle)
        taida_ossl.libssl_handle = dlopen("libssl.so", RTLD_LAZY);
    if (!taida_ossl.libssl_handle) return 0;

    taida_ossl.libcrypto_handle = dlopen("libcrypto.so.3", RTLD_LAZY);
    if (!taida_ossl.libcrypto_handle)
        taida_ossl.libcrypto_handle = dlopen("libcrypto.so", RTLD_LAZY);
    if (!taida_ossl.libcrypto_handle) {
        dlclose(taida_ossl.libssl_handle);
        taida_ossl.libssl_handle = NULL;
        return 0;
    }

    // Resolve symbols. Cast through void* to suppress -Wpedantic warnings.
    #define LOAD_SYM(lib, name) do { \
        *(void**)(&taida_ossl.name) = dlsym(taida_ossl.lib##_handle, #name); \
        if (!taida_ossl.name) { taida_ossl_unload(); return 0; } \
    } while(0)

    LOAD_SYM(libssl, TLS_server_method);
    LOAD_SYM(libssl, SSL_CTX_new);
    LOAD_SYM(libssl, SSL_CTX_free);
    LOAD_SYM(libssl, SSL_CTX_use_certificate_chain_file);
    LOAD_SYM(libssl, SSL_CTX_use_PrivateKey_file);
    LOAD_SYM(libssl, SSL_CTX_check_private_key);
    LOAD_SYM(libssl, SSL_new);
    LOAD_SYM(libssl, SSL_free);
    LOAD_SYM(libssl, SSL_set_fd);
    LOAD_SYM(libssl, SSL_accept);
    LOAD_SYM(libssl, SSL_read);
    LOAD_SYM(libssl, SSL_write);
    LOAD_SYM(libssl, SSL_shutdown);
    LOAD_SYM(libssl, SSL_get_error);
    LOAD_SYM(libssl, SSL_CTX_set_options);
    // ALPN symbols: these are optional — gracefully degrade if absent.
    // Server-side: SSL_CTX_set_alpn_select_cb + SSL_select_next_proto (added in OpenSSL 1.0.2).
    *(void**)(&taida_ossl.SSL_CTX_set_alpn_select_cb) = dlsym(taida_ossl.libssl_handle, "SSL_CTX_set_alpn_select_cb");
    *(void**)(&taida_ossl.SSL_select_next_proto) = dlsym(taida_ossl.libcrypto_handle, "SSL_select_next_proto");
    *(void**)(&taida_ossl.SSL_get0_alpn_selected) = dlsym(taida_ossl.libssl_handle, "SSL_get0_alpn_selected");
    // NULL ALPN pointers are checked before use; absent == no h2 ALPN support.

    #undef LOAD_SYM

    taida_ossl.loaded = 1;
    return 1;
}

static void taida_ossl_unload(void) {
    if (taida_ossl.libssl_handle) { dlclose(taida_ossl.libssl_handle); taida_ossl.libssl_handle = NULL; }
    if (taida_ossl.libcrypto_handle) { dlclose(taida_ossl.libcrypto_handle); taida_ossl.libcrypto_handle = NULL; }
    taida_ossl.loaded = 0;
}

// Create an SSL_CTX for TLS server with cert/key PEM files.
// Returns non-NULL on success. On failure, writes error to errbuf and returns NULL.
static OSSL_SSL_CTX *taida_tls_create_ctx(const char *cert_path, const char *key_path, char *errbuf, size_t errbuf_sz) {
    OSSL_SSL_CTX *ctx = taida_ossl.SSL_CTX_new(taida_ossl.TLS_server_method());
    if (!ctx) {
        snprintf(errbuf, errbuf_sz, "httpServe: failed to create SSL context");
        return NULL;
    }
    // Only allow TLS 1.2+.
    taida_ossl.SSL_CTX_set_options(ctx, TAIDA_SSL_OP_SECURE);

    if (taida_ossl.SSL_CTX_use_certificate_chain_file(ctx, cert_path) != 1) {
        snprintf(errbuf, errbuf_sz, "httpServe: failed to load cert file '%s'", cert_path);
        taida_ossl.SSL_CTX_free(ctx);
        return NULL;
    }
    if (taida_ossl.SSL_CTX_use_PrivateKey_file(ctx, key_path, TAIDA_SSL_FILETYPE_PEM) != 1) {
        snprintf(errbuf, errbuf_sz, "httpServe: failed to load key file '%s'", key_path);
        taida_ossl.SSL_CTX_free(ctx);
        return NULL;
    }
    if (taida_ossl.SSL_CTX_check_private_key(ctx) != 1) {
        snprintf(errbuf, errbuf_sz, "httpServe: cert/key mismatch for '%s' / '%s'", cert_path, key_path);
        taida_ossl.SSL_CTX_free(ctx);
        return NULL;
    }
    return ctx;
}

// ALPN server-side select callback: prefers "h2", falls back to "http/1.1".
// arg is unused.
#define TAIDA_OPENSSL_NPN_NEGOTIATED 0
static int taida_h2_alpn_select_cb(OSSL_SSL *ssl, const unsigned char **out, unsigned char *outlen,
                                    const unsigned char *in, unsigned int inlen, void *arg) {
    (void)ssl; (void)arg;
    // Server preference: h2 then http/1.1
    static const unsigned char server_protos[] = {
        0x02, 'h', '2',
        0x08, 'h', 't', 't', 'p', '/', '1', '.', '1'
    };
    if (taida_ossl.SSL_select_next_proto) {
        int rc = taida_ossl.SSL_select_next_proto(
            (unsigned char **)out, outlen,
            server_protos, sizeof(server_protos),
            in, inlen);
        if (rc == TAIDA_OPENSSL_NPN_NEGOTIATED) {
            return 0; // SSL_TLSEXT_ERR_OK
        }
    } else {
        // Fallback: manually scan client list for "h2"
        const unsigned char *p = in;
        const unsigned char *end = in + inlen;
        while (p < end) {
            unsigned char len = *p++;
            if (len == 2 && p + 2 <= end && p[0] == 'h' && p[1] == '2') {
                *out = p;
                *outlen = 2;
                return 0; // SSL_TLSEXT_ERR_OK
            }
            p += len;
        }
    }
    return 3; // SSL_TLSEXT_ERR_NOACK (no match, proceed without ALPN)
}

// Create an SSL_CTX for HTTP/2 server: cert/key + ALPN ["h2", "http/1.1"].
// Uses server-side SSL_CTX_set_alpn_select_cb for correct ALPN negotiation.
// Returns non-NULL on success.  On failure, writes error to errbuf and returns NULL.
static OSSL_SSL_CTX *taida_tls_create_ctx_h2(const char *cert_path, const char *key_path, char *errbuf, size_t errbuf_sz) {
    OSSL_SSL_CTX *ctx = taida_tls_create_ctx(cert_path, key_path, errbuf, errbuf_sz);
    if (!ctx) return NULL;

    // Register server-side ALPN selection callback.
    // This is what actually tells OpenSSL to respond to the client's ALPN extension
    // and select "h2". Without this, SSL_get0_alpn_selected() returns nothing.
    if (taida_ossl.SSL_CTX_set_alpn_select_cb) {
        taida_ossl.SSL_CTX_set_alpn_select_cb(ctx, taida_h2_alpn_select_cb, NULL);
    }
    return ctx;
}

// Thread-local: current SSL connection pointer for TLS-aware I/O.
// NULL = plaintext (v4 path), non-NULL = TLS connection.
static __thread OSSL_SSL *tl_ssl = NULL;

// ── TLS-aware I/O wrappers ──────────────────────────────────────────
// These check tl_ssl and route through SSL_read/SSL_write when active.

// TLS-aware recv: reads from SSL or raw fd. Returns bytes read, or <=0 on error/EOF.
static ssize_t taida_tls_recv(int fd, void *buf, size_t len) {
    if (tl_ssl) {
        int n = taida_ossl.SSL_read(tl_ssl, buf, (int)(len > INT_MAX ? INT_MAX : len));
        if (n <= 0) {
            int err = taida_ossl.SSL_get_error(tl_ssl, n);
            if (err == TAIDA_SSL_ERROR_ZERO_RETURN) return 0; // clean TLS shutdown
            if (err == TAIDA_SSL_ERROR_WANT_READ || err == TAIDA_SSL_ERROR_WANT_WRITE) {
                errno = EAGAIN;
                return -1;
            }
            errno = EIO;
            return -1;
        }
        return (ssize_t)n;
    }
    return recv(fd, buf, len, 0);
}

// TLS-aware send_all: writes all bytes through SSL or raw fd.
// Returns 0 on success, -1 on error.
static int taida_tls_send_all(int fd, const void *buf, size_t len) {
    const unsigned char *p = (const unsigned char*)buf;
    size_t remaining = len;
    if (tl_ssl) {
        while (remaining > 0) {
            int chunk = (int)(remaining > INT_MAX ? INT_MAX : remaining);
            int n = taida_ossl.SSL_write(tl_ssl, p, chunk);
            if (n <= 0) return -1;
            p += n;
            remaining -= (size_t)n;
        }
        return 0;
    }
    // Plaintext path: delegate to existing send_all.
    while (remaining > 0) {
        ssize_t n = send(fd, p, remaining, MSG_NOSIGNAL);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (n == 0) return -1;
        p += (size_t)n;
        remaining -= (size_t)n;
    }
    return 0;
}

// TLS-aware writev_all: writes all iov buffers through SSL or raw fd.
// For TLS, we linearize the iovecs since SSL_write doesn't support scatter/gather.
// Returns 0 on success, -1 on error.
static int taida_tls_writev_all(int fd, struct iovec *iov, int iovcnt) {
    if (tl_ssl) {
        // NB6-3: SSL doesn't support writev — linearize all iovecs into a single
        // contiguous buffer and make one SSL_write call. This prevents TLS record
        // fragmentation (previously one SSL_write per iovec caused 3 TLS records
        // per chunked response chunk). Stack buffer for small writes, heap fallback.
        size_t total = 0;
        for (int i = 0; i < iovcnt; i++) total += iov[i].iov_len;
        if (total == 0) return 0;
        // Single iovec: no linearization needed.
        if (iovcnt == 1) {
            return taida_tls_send_all(fd, iov[0].iov_base, iov[0].iov_len);
        }
        unsigned char stack_buf[8192];
        unsigned char *buf = (total <= sizeof(stack_buf)) ? stack_buf
            : (unsigned char*)TAIDA_MALLOC(total, "tls_writev_linear");
        // NB6-32: NULL check for heap allocation — OOM must not cause UB
        if (buf == NULL) return -1;
        size_t pos = 0;
        for (int i = 0; i < iovcnt; i++) {
            if (iov[i].iov_len > 0) {
                memcpy(buf + pos, iov[i].iov_base, iov[i].iov_len);
                pos += iov[i].iov_len;
            }
        }
        int rc = taida_tls_send_all(fd, buf, total);
        if (buf != stack_buf) free(buf);
        return rc;
    }
    // Plaintext: use real writev.
    while (iovcnt > 0) {
        ssize_t n = writev(fd, iov, iovcnt);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (n == 0) return -1;
        size_t written = (size_t)n;
        while (iovcnt > 0 && written >= iov[0].iov_len) {
            written -= iov[0].iov_len;
            iov++;
            iovcnt--;
        }
        if (iovcnt > 0 && written > 0) {
            iov[0].iov_base = (char*)iov[0].iov_base + written;
            iov[0].iov_len -= written;
        }
    }
    return 0;
}

// TLS-aware recv_exact: reads exactly `count` bytes. Returns bytes actually read.
static size_t taida_tls_recv_exact(int fd, unsigned char *out, size_t count) {
    size_t pos = 0;
    while (pos < count) {
        ssize_t n = taida_tls_recv(fd, out + pos, count - pos);
        if (n <= 0) {
            if (n < 0 && errno == EINTR) continue;
            return pos;
        }
        pos += (size_t)n;
    }
    return pos;
}

// Perform TLS handshake on an accepted fd. Returns SSL* on success, NULL on failure.
static OSSL_SSL *taida_tls_handshake(OSSL_SSL_CTX *ctx, int fd) {
    OSSL_SSL *ssl = taida_ossl.SSL_new(ctx);
    if (!ssl) return NULL;
    if (taida_ossl.SSL_set_fd(ssl, fd) != 1) {
        taida_ossl.SSL_free(ssl);
        return NULL;
    }
    int ret = taida_ossl.SSL_accept(ssl);
    if (ret != 1) {
        // Handshake failed — connection close per NET5-0c policy.
        taida_ossl.SSL_free(ssl);
        return NULL;
    }
    return ssl;
}

// TLS shutdown + free.
static void taida_tls_shutdown_free(OSSL_SSL *ssl) {
    if (!ssl) return;
    taida_ossl.SSL_shutdown(ssl);
    taida_ossl.SSL_free(ssl);
}

// Helper: create a resolved Async[value] (fulfilled)
// NO-3: auto-detect value type for ownership tracking
static taida_val taida_async_resolved(taida_val value) {
    taida_val vtag = taida_detect_value_tag(value);
    return taida_async_ok_tagged(value, vtag);
}

// ── ReadAsync[path]() → Async[Lax[Str]] ──────────────────
// Synchronous implementation wrapped in Async (pthread spawn for true async is future work)
taida_val taida_os_read_async(taida_val path_ptr) {
    // Reuse the sync Read implementation, wrap in Async
    taida_val lax_result = taida_os_read(path_ptr);
    return taida_async_resolved(lax_result);
}

// ── HTTP helpers (minimal HTTP/1.1 over raw TCP) ─────────
// FNV-1a hashes: "status", "body", "headers"
#define TAIDA_HTTP_STATUS_HASH  0xc4d5696d6cc12c2fULL
#define TAIDA_HTTP_BODY_HASH    0xcd4de79bc6c93295ULL
#define TAIDA_HTTP_HEADERS_HASH 0x8cc1ca917bac9b49ULL

static taida_val taida_os_http_default_response(void) {
    taida_val result = taida_pack_new(3);
    taida_pack_set_hash(result, 0, (taida_val)TAIDA_HTTP_STATUS_HASH);
    taida_pack_set(result, 0, 0);
    taida_pack_set_hash(result, 1, (taida_val)TAIDA_HTTP_BODY_HASH);
    taida_pack_set(result, 1, (taida_val)"");
    taida_pack_set_hash(result, 2, (taida_val)TAIDA_HTTP_HEADERS_HASH);
    taida_pack_set(result, 2, taida_pack_new(0));
    return result;
}

static taida_val taida_os_http_failure_lax(void) {
    taida_val error = taida_make_error_with_kind_code("IoError", "HttpRequest error", "other", 0);
    return taida_lax_empty_error(taida_os_http_default_response(), error);
}

static taida_val taida_os_http_failure_lax_kind(const char *kind) {
    taida_val error = taida_make_error_with_kind_code(
        "IoError",
        "HttpRequest error",
        kind ? kind : "other",
        0
    );
    return taida_lax_empty_error(taida_os_http_default_response(), error);
}

/*
 * C20-4 (C19B-007): append one wire header to the growing `buf` in
 * "Name: Value\r\n" form, stripping CR/LF from both sides to keep the
 * RCB-304 CRLF-injection guard intact. Used by both shapes
 * (BuchiPack / List-of-record) so the raw HTTP and curl paths share
 * the same sanitization.
 */
static int taida_os_http_append_header_line(
    char **buf,
    size_t *cap,
    size_t *len,
    const char *name,
    const char *value
) {
    if (!name || !*name) return 1; // skip empty name silently
    if (!value) value = "";

    char *safe_name = strdup(name);
    char *safe_value = strdup(value);
    if (!safe_name || !safe_value) {
        free(safe_name);
        free(safe_value);
        return 0;
    }
    for (char *p = safe_name; *p; p++) { if (*p == '\r' || *p == '\n') *p = ' '; }
    for (char *p = safe_value; *p; p++) { if (*p == '\r' || *p == '\n') *p = ' '; }

    size_t need = strlen(safe_name) + strlen(safe_value) + 4;
    while (*len + need + 1 > *cap) {
        *cap *= 2;
        TAIDA_REALLOC(*buf, *cap, "http_response");
    }

    int n = snprintf(*buf + *len, *cap - *len, "%s: %s\r\n", safe_name, safe_value);
    free(safe_name);
    free(safe_value);
    if (n < 0) return 0;
    *len += (size_t)n;
    return 1;
}

/*
 * C20-4 (C19B-007): extract a Str field value from a BuchiPack given
 * the pre-hashed field name. Returns a malloc'd copy (caller frees)
 * or NULL if the field is absent / not a Str.
 */
static char *taida_os_http_pack_str_field(taida_val pack_ptr, taida_val field_hash) {
    if (!pack_ptr || !taida_is_buchi_pack(pack_ptr)) return NULL;
    taida_val *pack = (taida_val*)pack_ptr;
    taida_val fc = pack[1];
    for (taida_val i = 0; i < fc; i++) {
        if (pack[2 + i * 3] == field_hash) {
            taida_val value_str_ptr = taida_value_to_display_string(pack[2 + i * 3 + 2]);
            const char *s = (const char*)value_str_ptr;
            char *copy = s ? strdup(s) : NULL;
            taida_str_release(value_str_ptr);
            return copy;
        }
    }
    return NULL;
}

static char *taida_os_http_headers_to_lines(taida_val headers_ptr) {
    size_t cap = 128;
    size_t len = 0;
    char *buf = (char*)TAIDA_MALLOC(cap, "http_headers");
    buf[0] = '\0';

    if (!headers_ptr) return buf;

    if (taida_is_buchi_pack(headers_ptr)) {
        taida_val *pack = (taida_val*)headers_ptr;
        taida_val fc = pack[1];
        for (taida_val i = 0; i < fc; i++) {
            taida_val field_hash = pack[2 + i * 3];
            taida_val field_val = pack[2 + i * 3 + 2];
            const char *name = taida_lookup_field_name(field_hash);
            if (!name || !name[0]) continue;

            taida_val value_str_ptr = taida_value_to_display_string(field_val);
            const char *value_str = (const char*)value_str_ptr;
            int ok = taida_os_http_append_header_line(&buf, &cap, &len, name, value_str);
            taida_str_release(value_str_ptr);
            if (!ok) {
                free(buf);
                char *empty = (char*)TAIDA_MALLOC(1, "http_headers_err");
                empty[0] = '\0';
                return empty;
            }
        }
        return buf;
    }

    // C20-4: list-of-record shape `@[@(name <= "...", value <= "...")]`.
    if (taida_is_list(headers_ptr)) {
        taida_val count = taida_list_length(headers_ptr);
        taida_val *list = (taida_val*)headers_ptr;
        taida_val name_hash = taida_str_hash((taida_val)"name");
        taida_val value_hash = taida_str_hash((taida_val)"value");
        for (taida_val i = 0; i < count; i++) {
            taida_val rec = list[4 + i];
            char *hn = taida_os_http_pack_str_field(rec, name_hash);
            char *hv = taida_os_http_pack_str_field(rec, value_hash);
            if (hn && hv && hn[0]) {
                (void)taida_os_http_append_header_line(&buf, &cap, &len, hn, hv);
            }
            free(hn);
            free(hv);
        }
    }
    return buf;
}

static taida_val taida_os_http_parse_headers(const char *header_start, const char *header_end) {
    if (!header_start || !header_end || header_end <= header_start) return taida_pack_new(0);

    const char *lines_start = strstr(header_start, "\r\n");
    if (!lines_start || lines_start >= header_end) return taida_pack_new(0);
    lines_start += 2; // skip status line

    size_t header_count = 0;
    const char *scan = lines_start;
    while (scan < header_end) {
        const char *line_end = strstr(scan, "\r\n");
        if (!line_end || line_end > header_end) line_end = header_end;
        const char *colon = memchr(scan, ':', (size_t)(line_end - scan));
        if (colon) header_count++;
        if (line_end >= header_end) break;
        scan = line_end + 2;
    }

    taida_val headers_pack = taida_pack_new((taida_val)header_count);
    taida_val idx = 0;
    scan = lines_start;
    while (scan < header_end && idx < (taida_val)header_count) {
        const char *line_end = strstr(scan, "\r\n");
        if (!line_end || line_end > header_end) line_end = header_end;
        const char *colon = memchr(scan, ':', (size_t)(line_end - scan));
        if (colon) {
            size_t key_len = (size_t)(colon - scan);
            char *key = (char*)TAIDA_MALLOC(key_len + 1, "http_header_key");
            for (size_t i = 0; i < key_len; i++) {
                char c = scan[i];
                if (c >= 'A' && c <= 'Z') c = (char)(c + 32);
                key[i] = c;
            }
            key[key_len] = '\0';

            const char *value_start = colon + 1;
            while (value_start < line_end && (*value_start == ' ' || *value_start == '\t')) value_start++;
            size_t value_len = (size_t)(line_end - value_start);
            char *value = (char*)TAIDA_MALLOC(value_len + 1, "http_header_value");
            memcpy(value, value_start, value_len);
            value[value_len] = '\0';

            taida_val key_hash = taida_str_hash((taida_val)key);
            taida_register_field_name(key_hash, (taida_val)key);
            taida_register_field_type(key_hash, (taida_val)key, 3);
            taida_pack_set_hash(headers_pack, idx, key_hash);
            char *value_str = taida_str_new_copy(value);
            free(value);
            taida_pack_set(headers_pack, idx, (taida_val)value_str);
            taida_pack_set_tag(headers_pack, idx, TAIDA_TAG_STR);
            idx++;
        }

        if (line_end >= header_end) break;
        scan = line_end + 2;
    }

    return headers_pack;
}

static int taida_os_cmd_append(char **buf, size_t *cap, size_t *len, const char *chunk) {
    if (!chunk) return 1;
    size_t n = strlen(chunk);
    if (*len + n + 1 > *cap) {
        size_t new_cap = *cap;
        while (*len + n + 1 > new_cap) new_cap *= 2;
        char *new_buf = (char*)realloc(*buf, new_cap);
        if (!new_buf) return 0;
        *buf = new_buf;
        *cap = new_cap;
    }
    memcpy(*buf + *len, chunk, n);
    *len += n;
    (*buf)[*len] = '\0';
    return 1;
}

static char *taida_os_shell_quote(const char *s) {
    if (!s) s = "";
    size_t out_len = 2; // surrounding single quotes
    for (const char *p = s; *p; p++) {
        out_len += (*p == '\'') ? 4 : 1; // '\'' sequence for single quote
    }

    char *out = (char*)malloc(out_len + 1);
    if (!out) return NULL;
    char *w = out;
    *w++ = '\'';
    for (const char *p = s; *p; p++) {
        if (*p == '\'') {
            memcpy(w, "'\\''", 4);
            w += 4;
        } else {
            *w++ = *p;
        }
    }
    *w++ = '\'';
    *w = '\0';
    return out;
}

static taida_val taida_os_http_do_curl(const char *method, const char *url, taida_val headers_ptr, const char *body) {
    const char *method_str = (method && *method) ? method : "GET";
    const char *url_str = url ? url : "";
    const char *body_str = body ? body : "";

    char *q_method = taida_os_shell_quote(method_str);
    char *q_url = taida_os_shell_quote(url_str);
    if (!q_method || !q_url) {
        free(q_method);
        free(q_url);
        return taida_async_resolved(taida_os_http_failure_lax());
    }

    size_t cmd_cap = 1024;
    size_t cmd_len = 0;
    char *cmd = (char*)malloc(cmd_cap);
    if (!cmd) {
        free(q_method);
        free(q_url);
        return taida_async_resolved(taida_os_http_failure_lax());
    }
    cmd[0] = '\0';

    // RCB-306: Limit response size for HTTPS (curl) path — 100MB matches raw HTTP limit
    if (!taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, "curl -sS -i --max-time 30 --max-filesize 104857600 -X ")
        || !taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, q_method)
        || !taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, " ")
        || !taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, q_url)) {
        free(cmd);
        free(q_method);
        free(q_url);
        return taida_async_resolved(taida_os_http_failure_lax());
    }
    free(q_method);
    free(q_url);

    // C20-4 (C19B-007): render each wire header via `-H 'Name: Value'`.
    // Accept both BuchiPack (legacy) and list-of-record
    // (`@[@(name <= "x-api-key", value <= "...")]`) shapes.
    {
        // Small helper closure-esque block: format one safe `-H` arg.
        #define C20_HTTP_ADD_CURL_HEADER(name_cstr, value_cstr) do { \
            const char *hn = (name_cstr); \
            const char *hv = (value_cstr) ? (value_cstr) : ""; \
            if (hn && hn[0]) { \
                size_t _hv_len = strlen(hn) + strlen(hv) + 2; \
                char *_pair = (char*)malloc(_hv_len + 1); \
                if (_pair) { \
                    snprintf(_pair, _hv_len + 1, "%s: %s", hn, hv); \
                    char *_q = taida_os_shell_quote(_pair); \
                    free(_pair); \
                    if (_q) { \
                        (void)taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, " -H "); \
                        (void)taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, _q); \
                        free(_q); \
                    } \
                } \
            } \
        } while (0)

        if (headers_ptr && taida_is_buchi_pack(headers_ptr)) {
            taida_val *pack = (taida_val*)headers_ptr;
            taida_val fc = pack[1];
            for (taida_val i = 0; i < fc; i++) {
                taida_val field_hash = pack[2 + i * 3];
                taida_val field_val = pack[2 + i * 3 + 2];
                const char *name = taida_lookup_field_name(field_hash);
                if (!name || !name[0]) continue;

                taida_val value_str_ptr = taida_value_to_display_string(field_val);
                const char *value_str = (const char*)value_str_ptr;
                C20_HTTP_ADD_CURL_HEADER(name, value_str);
                taida_str_release(value_str_ptr);
            }
        } else if (headers_ptr && taida_is_list(headers_ptr)) {
            taida_val count = taida_list_length(headers_ptr);
            taida_val *list = (taida_val*)headers_ptr;
            taida_val name_hash = taida_str_hash((taida_val)"name");
            taida_val value_hash = taida_str_hash((taida_val)"value");
            for (taida_val i = 0; i < count; i++) {
                taida_val rec = list[4 + i];
                char *hn = NULL;
                char *hv = NULL;
                hn = taida_os_http_pack_str_field(rec, name_hash);
                hv = taida_os_http_pack_str_field(rec, value_hash);
                if (hn && hv) {
                    C20_HTTP_ADD_CURL_HEADER(hn, hv);
                }
                free(hn);
                free(hv);
            }
        }
        #undef C20_HTTP_ADD_CURL_HEADER
    }

    if (body_str[0] != '\0') {
        char *q_body = taida_os_shell_quote(body_str);
        if (!q_body) {
            free(cmd);
            return taida_async_resolved(taida_os_http_failure_lax());
        }
        int ok = taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, " --data-raw ")
            && taida_os_cmd_append(&cmd, &cmd_cap, &cmd_len, q_body);
        free(q_body);
        if (!ok) {
            free(cmd);
            return taida_async_resolved(taida_os_http_failure_lax());
        }
    }

    FILE *fp = popen(cmd, "r");
    free(cmd);
    if (!fp) return taida_async_resolved(taida_os_http_failure_lax());

    size_t resp_cap = 65536;
    size_t resp_len = 0;
    char *resp_buf = (char*)malloc(resp_cap);
    if (!resp_buf) {
        pclose(fp);
        return taida_async_resolved(taida_os_http_failure_lax());
    }

    size_t n;
    while ((n = fread(resp_buf + resp_len, 1, resp_cap - resp_len - 1, fp)) > 0) {
        resp_len += n;
        if (resp_len >= resp_cap - 1) {
            resp_cap *= 2;
            char *new_buf = (char*)realloc(resp_buf, resp_cap);
            if (!new_buf) {
                free(resp_buf);
                pclose(fp);
                return taida_async_resolved(taida_os_http_failure_lax());
            }
            resp_buf = new_buf;
        }
    }
    resp_buf[resp_len] = '\0';

    int status = pclose(fp);
    if (status == -1 || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        free(resp_buf);
        return taida_async_resolved(taida_os_http_failure_lax());
    }

    char *header_end = strstr(resp_buf, "\r\n\r\n");
    if (!header_end) {
        free(resp_buf);
        return taida_async_resolved(taida_os_http_failure_lax());
    }

    int status_code = 0;
    if (resp_len > 12 && resp_buf[0] == 'H') {
        char *sp = strchr(resp_buf, ' ');
        if (sp) status_code = atoi(sp + 1);
    }

    char *resp_body = header_end + 4;
    size_t resp_body_len = resp_len - (size_t)(resp_body - resp_buf);
    char *body_copy = (char*)malloc(resp_body_len + 1);
    if (!body_copy) {
        free(resp_buf);
        return taida_async_resolved(taida_os_http_failure_lax());
    }
    memcpy(body_copy, resp_body, resp_body_len);
    body_copy[resp_body_len] = '\0';

    taida_val headers_pack = taida_os_http_parse_headers(resp_buf, header_end);

    taida_val result = taida_pack_new(3);
    taida_pack_set_hash(result, 0, (taida_val)TAIDA_HTTP_STATUS_HASH);
    taida_pack_set(result, 0, (taida_val)status_code);
    taida_pack_set_hash(result, 1, (taida_val)TAIDA_HTTP_BODY_HASH);
    taida_pack_set(result, 1, (taida_val)body_copy);
    taida_pack_set_hash(result, 2, (taida_val)TAIDA_HTTP_HEADERS_HASH);
    taida_pack_set(result, 2, headers_pack);

    free(resp_buf);
    return taida_async_resolved(taida_lax_new(result, taida_os_http_default_response()));
}

static taida_val taida_os_http_do(const char *method, const char *url, taida_val headers_ptr, const char *body) {
    if (!url) return taida_async_resolved(taida_os_http_failure_lax_kind("invalid"));

    const char *scheme_end = strstr(url, "://");
    int use_tls = 0;
    const char *host_start;
    if (scheme_end) {
        if (strncmp(url, "https://", 8) == 0) {
            use_tls = 1;
        } else if (strncmp(url, "http://", 7) != 0) {
            return taida_async_resolved(taida_os_http_failure_lax_kind("invalid"));
        }
        host_start = scheme_end + 3;
    } else {
        host_start = url;
    }

    // HTTPS: route via curl TLS transport.
    if (use_tls) return taida_os_http_do_curl(method, url, headers_ptr, body);

    char host_buf[256] = {0};
    int port = 80;
    const char *path = "/";

    const char *slash = strchr(host_start, '/');
    const char *colon = strchr(host_start, ':');
    size_t host_len;

    if (slash) {
        path = slash;
        if (colon && colon < slash) {
            host_len = (size_t)(colon - host_start);
            port = atoi(colon + 1);
        } else {
            host_len = (size_t)(slash - host_start);
        }
    } else {
        if (colon) {
            host_len = (size_t)(colon - host_start);
            port = atoi(colon + 1);
        } else {
            host_len = strlen(host_start);
        }
    }

    if (host_len >= sizeof(host_buf)) host_len = sizeof(host_buf) - 1;
    memcpy(host_buf, host_start, host_len);
    host_buf[host_len] = '\0';

    // RCB-304: Reject URLs with CR/LF in host or path to prevent CRLF injection
    if (strchr(host_buf, '\r') || strchr(host_buf, '\n') ||
        strchr(path, '\r') || strchr(path, '\n')) {
        return taida_async_resolved(taida_os_http_failure_lax_kind("invalid"));
    }

    struct addrinfo hints = {0}, *res = NULL;
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    char port_str[16];
    snprintf(port_str, sizeof(port_str), "%d", port);
    if (getaddrinfo(host_buf, port_str, &hints, &res) != 0 || !res) {
        return taida_async_resolved(taida_os_http_failure_lax_kind("dns"));
    }

    int sockfd = socket(res->ai_family, res->ai_socktype, res->ai_protocol);
    if (sockfd < 0) {
        freeaddrinfo(res);
        return taida_async_resolved(taida_os_http_failure_lax_kind(taida_os_error_kind(errno, strerror(errno))));
    }

    if (connect(sockfd, res->ai_addr, res->ai_addrlen) < 0) {
        int saved_errno = errno;
        close(sockfd);
        freeaddrinfo(res);
        return taida_async_resolved(taida_os_http_failure_lax_kind(taida_os_error_kind(saved_errno, strerror(saved_errno))));
    }
    freeaddrinfo(res);

    char *header_lines = taida_os_http_headers_to_lines(headers_ptr);
    const char *method_str = (method && *method) ? method : "GET";
    const char *body_str = body ? body : "";
    size_t body_len = strlen(body_str);
    size_t header_lines_len = strlen(header_lines);
    size_t req_cap = strlen(method_str) + strlen(path) + strlen(host_buf) + header_lines_len + body_len + 256;
    char *request = (char*)TAIDA_MALLOC(req_cap, "http_request");

    int req_len;
    if (body_len > 0) {
        req_len = snprintf(
            request, req_cap,
            "%s %s HTTP/1.1\r\nHost: %s\r\nConnection: close\r\nContent-Length: %zu\r\nContent-Type: text/plain\r\n%s\r\n%s",
            method_str, path, host_buf, body_len, header_lines, body_str
        );
    } else {
        req_len = snprintf(
            request, req_cap,
            "%s %s HTTP/1.1\r\nHost: %s\r\nConnection: close\r\n%s\r\n",
            method_str, path, host_buf, header_lines
        );
    }
    free(header_lines);

    if (req_len < 0 || (size_t)req_len >= req_cap) {
        free(request);
        close(sockfd);
        return taida_async_resolved(taida_os_http_failure_lax_kind("invalid"));
    }

    size_t sent_total = 0;
    while (sent_total < (size_t)req_len) {
        ssize_t sent = send(sockfd, request + sent_total, (size_t)req_len - sent_total, MSG_NOSIGNAL);
        if (sent <= 0) {
            free(request);
            close(sockfd);
            return taida_async_resolved(taida_os_http_failure_lax_kind(taida_os_error_kind(errno, strerror(errno))));
        }
        sent_total += (size_t)sent;
    }
    free(request);

    /* RCB-306: Limit HTTP response to 100 MB to prevent OOM */
    const size_t MAX_HTTP_RESPONSE = 100 * 1024 * 1024;
    size_t buf_cap = 65536;
    char *resp_buf = (char*)TAIDA_MALLOC(buf_cap, "http_recv");
    size_t resp_len = 0;
    ssize_t n;
    while ((n = recv(sockfd, resp_buf + resp_len, buf_cap - resp_len - 1, 0)) > 0) {
        resp_len += (size_t)n;
        if (resp_len > MAX_HTTP_RESPONSE) {
            close(sockfd);
            free(resp_buf);
            return taida_async_resolved(taida_os_http_failure_lax_kind("too_large"));
        }
        if (resp_len >= buf_cap - 1) {
            buf_cap *= 2;
            if (buf_cap > MAX_HTTP_RESPONSE + 1) buf_cap = MAX_HTTP_RESPONSE + 1;
            TAIDA_REALLOC(resp_buf, buf_cap, "tcp_recv");
        }
    }
    close(sockfd);
    resp_buf[resp_len] = '\0';

    char *header_end = strstr(resp_buf, "\r\n\r\n");
    if (!header_end) {
        free(resp_buf);
        return taida_async_resolved(taida_os_http_failure_lax_kind("invalid"));
    }

    int status_code = 0;
    if (resp_len > 12 && resp_buf[0] == 'H') {
        char *sp = strchr(resp_buf, ' ');
        if (sp) status_code = atoi(sp + 1);
    }

    char *resp_body = header_end + 4;
    size_t resp_body_len = resp_len - (size_t)(resp_body - resp_buf);
    char *body_copy = (char*)TAIDA_MALLOC(resp_body_len + 1, "http_body");
    memcpy(body_copy, resp_body, resp_body_len);
    body_copy[resp_body_len] = '\0';

    taida_val headers_pack = taida_os_http_parse_headers(resp_buf, header_end);

    taida_val result = taida_pack_new(3);
    taida_pack_set_hash(result, 0, (taida_val)TAIDA_HTTP_STATUS_HASH);
    taida_pack_set(result, 0, (taida_val)status_code);
    taida_pack_set_hash(result, 1, (taida_val)TAIDA_HTTP_BODY_HASH);
    taida_pack_set(result, 1, (taida_val)body_copy);
    taida_pack_set_hash(result, 2, (taida_val)TAIDA_HTTP_HEADERS_HASH);
    taida_pack_set(result, 2, headers_pack);

    free(resp_buf);
    return taida_async_resolved(taida_lax_new(result, taida_os_http_default_response()));
}

taida_val taida_os_http_get(taida_val url_ptr) {
    return taida_os_http_do("GET", (const char*)url_ptr, 0, NULL);
}

taida_val taida_os_http_post(taida_val url_ptr, taida_val body_ptr) {
    return taida_os_http_do("POST", (const char*)url_ptr, 0, (const char*)body_ptr);
}

taida_val taida_os_http_request(taida_val method_ptr, taida_val url_ptr, taida_val headers_ptr, taida_val body_ptr) {
    const char *method = (const char*)method_ptr;
    if (!method || !*method) method = "GET";
    return taida_os_http_do(method, (const char*)url_ptr, headers_ptr, (const char*)body_ptr);
}

// ── TCP socket APIs ───────────────────────────────────────

static taida_val taida_os_network_timeout_ms(taida_val timeout_ms) {
    if (timeout_ms <= 0 || timeout_ms > 600000) return 30000;
    return timeout_ms;
}

static void taida_os_apply_socket_timeout(int fd, taida_val timeout_ms) {
    taida_val ms = taida_os_network_timeout_ms(timeout_ms);
    struct timeval tv;
    tv.tv_sec = (time_t)(ms / 1000);
    tv.tv_usec = (suseconds_t)((ms % 1000) * 1000);
    (void)setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));
    (void)setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
}

static taida_val taida_os_dns_failure(const char *op_name, int gai_code) {
    char msg[256];
    if (gai_code != 0) {
        snprintf(msg, sizeof(msg), "%s: %s", op_name, gai_strerror(gai_code));
    } else {
        snprintf(msg, sizeof(msg), "%s: DNS resolution failed", op_name);
    }
    return taida_async_resolved(taida_os_result_failure(EINVAL, msg));
}

taida_val taida_os_dns_resolve(taida_val host_ptr, taida_val timeout_ms) {
    (void)timeout_ms; // getaddrinfo timeout is not configurable per-call in this runtime path.

    const char *host = (const char*)host_ptr;
    if (!host || !host[0]) {
        return taida_async_resolved(taida_os_result_failure(EINVAL, "dnsResolve: invalid host"));
    }

    struct addrinfo hints = {0}, *res = NULL;
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    int gai = getaddrinfo(host, NULL, &hints, &res);
    if (gai != 0 || !res) {
        return taida_os_dns_failure("dnsResolve", gai);
    }

    taida_val addresses = taida_list_new();
    for (struct addrinfo *it = res; it; it = it->ai_next) {
        char ip_buf[INET6_ADDRSTRLEN] = {0};
        const char *ip = NULL;

        if (it->ai_family == AF_INET) {
            struct sockaddr_in *addr4 = (struct sockaddr_in*)it->ai_addr;
            ip = inet_ntop(AF_INET, &addr4->sin_addr, ip_buf, sizeof(ip_buf));
        } else if (it->ai_family == AF_INET6) {
            struct sockaddr_in6 *addr6 = (struct sockaddr_in6*)it->ai_addr;
            ip = inet_ntop(AF_INET6, &addr6->sin6_addr, ip_buf, sizeof(ip_buf));
        }

        if (!ip || !ip[0]) continue;

        int exists = 0;
        taida_val len = taida_list_length(addresses);
        taida_val *list_ptr = (taida_val*)addresses;
        for (taida_val i = 0; i < len; i++) {
            const char *prev = (const char*)list_ptr[4 + i];
            if (prev && strcmp(prev, ip) == 0) {
                exists = 1;
                break;
            }
        }
        if (exists) continue;

        char *copy = taida_str_new_copy(ip);
        addresses = taida_list_push(addresses, (taida_val)copy);
    }
    freeaddrinfo(res);

    if (taida_list_length(addresses) <= 0) {
        return taida_os_dns_failure("dnsResolve", 0);
    }

    taida_val inner = taida_pack_new(1);
    taida_val addresses_hash = taida_str_hash((taida_val)"addresses");
    taida_pack_set_hash(inner, 0, addresses_hash);
    taida_pack_set(inner, 0, addresses);
    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_tcp_connect(taida_val host_ptr, taida_val port, taida_val timeout_ms) {
    const char *host = (const char*)host_ptr;
    if (!host) return taida_async_resolved(taida_os_result_failure(EINVAL, "tcpConnect: invalid host"));

    struct addrinfo hints = {0}, *res = NULL;
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    char port_str[16];
    snprintf(port_str, sizeof(port_str), "%" PRId64 "", port);
    int gai = getaddrinfo(host, port_str, &hints, &res);
    if (gai != 0 || !res) {
        return taida_os_dns_failure("tcpConnect", gai);
    }

    int sockfd = socket(res->ai_family, res->ai_socktype, res->ai_protocol);
    if (sockfd < 0) {
        freeaddrinfo(res);
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_os_apply_socket_timeout(sockfd, timeout_ms);
    if (connect(sockfd, res->ai_addr, res->ai_addrlen) < 0) {
        close(sockfd);
        freeaddrinfo(res);
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }
    freeaddrinfo(res);

    // Return @(socket: fd, host: host, port: port)
    taida_val inner = taida_pack_new(3);
    taida_val socket_hash = 0x10f2dcb841372d0cULL;
    taida_pack_set_hash(inner, 0, (taida_val)socket_hash);
    taida_pack_set(inner, 0, (taida_val)sockfd);
    taida_val host_hash = 0x4077f8cc7eaf4d6fULL;
    taida_pack_set_hash(inner, 1, (taida_val)host_hash);
    char *host_copy = taida_str_new_copy(host);
    taida_pack_set(inner, 1, (taida_val)host_copy);
    taida_pack_set_tag(inner, 1, TAIDA_TAG_STR);
    taida_val port_hash = 0x8c2cdb0da8933fa6ULL;
    taida_pack_set_hash(inner, 2, (taida_val)port_hash);
    taida_pack_set(inner, 2, port);

    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_tcp_listen(taida_val port, taida_val timeout_ms) {
    int sockfd = socket(AF_INET, SOCK_STREAM, 0);
    if (sockfd < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_os_apply_socket_timeout(sockfd, timeout_ms);

    int opt = 1;
    setsockopt(sockfd, SOL_SOCKET, SO_REUSEADDR, &opt, sizeof(opt));

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    /* RCB-305: Default to loopback (127.0.0.1) instead of all interfaces */
    inet_pton(AF_INET, "127.0.0.1", &addr.sin_addr);
    addr.sin_port = htons((unsigned short)port);

    if (bind(sockfd, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        close(sockfd);
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    if (listen(sockfd, 128) < 0) {
        close(sockfd);
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_val inner = taida_pack_new(2);
    taida_val listener_hash = 0x5a2d194b8a8ae591ULL;
    taida_pack_set_hash(inner, 0, (taida_val)listener_hash);
    taida_pack_set(inner, 0, (taida_val)sockfd);
    taida_val port_hash = 0x8c2cdb0da8933fa6ULL;
    taida_pack_set_hash(inner, 1, (taida_val)port_hash);
    taida_pack_set(inner, 1, port);

    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_tcp_accept(taida_val listener_fd, taida_val timeout_ms) {
    struct sockaddr_in peer_addr;
    socklen_t peer_len = sizeof(peer_addr);
    taida_os_apply_socket_timeout((int)listener_fd, timeout_ms);
    int client_fd = accept((int)listener_fd, (struct sockaddr*)&peer_addr, &peer_len);
    if (client_fd < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_os_apply_socket_timeout(client_fd, timeout_ms);

    char host_buf[INET_ADDRSTRLEN] = {0};
    const char *peer_host = inet_ntop(AF_INET, &peer_addr.sin_addr, host_buf, sizeof(host_buf));
    if (!peer_host) peer_host = "";
    taida_val peer_port = (taida_val)ntohs(peer_addr.sin_port);

    taida_val inner = taida_pack_new(3);
    taida_val socket_hash = taida_str_hash((taida_val)"socket");
    taida_val host_hash = taida_str_hash((taida_val)"host");
    taida_val port_hash = taida_str_hash((taida_val)"port");
    taida_pack_set_hash(inner, 0, socket_hash);
    taida_pack_set(inner, 0, (taida_val)client_fd);
    taida_pack_set_hash(inner, 1, host_hash);
    taida_pack_set(inner, 1, (taida_val)taida_str_new_copy(peer_host));
    taida_pack_set_tag(inner, 1, TAIDA_TAG_STR);
    taida_pack_set_hash(inner, 2, port_hash);
    taida_pack_set(inner, 2, peer_port);

    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_socket_send(taida_val socket_fd, taida_val data_ptr, taida_val timeout_ms) {
    unsigned char *payload_buf = NULL;
    size_t payload_len = 0;
    if (TAIDA_IS_BYTES(data_ptr)) {
        taida_val *bytes = (taida_val*)data_ptr;
        taida_val len = bytes[1];
        if (len < 0) return taida_async_resolved(taida_os_result_failure(EINVAL, "socketSend: invalid data"));
        // M-15: Cap bytes len to 256MB to prevent unbounded malloc.
        if (len > (taida_val)(256 * 1024 * 1024)) return taida_async_resolved(taida_os_result_failure(EINVAL, "socketSend: payload too large"));
        payload_buf = (unsigned char*)TAIDA_MALLOC((size_t)len, "socketSend_bytes");
        for (taida_val i = 0; i < len; i++) payload_buf[i] = (unsigned char)bytes[2 + i];
        payload_len = (size_t)len;
    } else {
        const char *data = (const char*)data_ptr;
        size_t data_len = 0;
        if (!taida_read_cstr_len_safe(data, 65536, &data_len)) {
            return taida_async_resolved(taida_os_result_failure(EINVAL, "socketSend: invalid data"));
        }
        payload_buf = (unsigned char*)TAIDA_MALLOC(data_len, "socketSend_str");
        memcpy(payload_buf, data, data_len);
        payload_len = data_len;
    }

    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);
    ssize_t sent = send((int)socket_fd, payload_buf, payload_len, MSG_NOSIGNAL);
    free(payload_buf);
    if (sent < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_val inner = taida_pack_new(2);
    taida_val ok_hash = 0x08b05d07b5566befULL;
    taida_pack_set_hash(inner, 0, (taida_val)ok_hash);
    taida_pack_set(inner, 0, 1);
    taida_val bytes_hash = 0x67ec7cd6a574048aULL;
    taida_pack_set_hash(inner, 1, (taida_val)bytes_hash);
    taida_pack_set(inner, 1, sent);

    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_socket_send_all(taida_val socket_fd, taida_val data_ptr, taida_val timeout_ms) {
    unsigned char *payload_buf = NULL;
    size_t payload_len = 0;
    if (TAIDA_IS_BYTES(data_ptr)) {
        taida_val *bytes = (taida_val*)data_ptr;
        taida_val len = bytes[1];
        if (len < 0) return taida_async_resolved(taida_os_result_failure(EINVAL, "socketSendAll: invalid data"));
        // M-15: Cap bytes len to 256MB to prevent unbounded malloc.
        if (len > (taida_val)(256 * 1024 * 1024)) return taida_async_resolved(taida_os_result_failure(EINVAL, "socketSendAll: payload too large"));
        payload_buf = (unsigned char*)TAIDA_MALLOC((size_t)len, "socketSendAll_bytes");
        for (taida_val i = 0; i < len; i++) payload_buf[i] = (unsigned char)bytes[2 + i];
        payload_len = (size_t)len;
    } else {
        const char *data = (const char*)data_ptr;
        size_t data_len = 0;
        if (!taida_read_cstr_len_safe(data, 65536, &data_len)) {
            return taida_async_resolved(taida_os_result_failure(EINVAL, "socketSendAll: invalid data"));
        }
        payload_buf = (unsigned char*)TAIDA_MALLOC(data_len, "socketSendAll_payload");
        memcpy(payload_buf, data, data_len);
        payload_len = data_len;
    }

    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);
    size_t sent_total = 0;
    while (sent_total < payload_len) {
        ssize_t sent = send((int)socket_fd, payload_buf + sent_total, payload_len - sent_total, MSG_NOSIGNAL);
        if (sent < 0) {
            if (errno == EINTR) continue;
            free(payload_buf);
            return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
        }
        if (sent == 0) {
            free(payload_buf);
            return taida_async_resolved(
                taida_os_result_failure(EPIPE, "socketSendAll: peer closed while sending")
            );
        }
        sent_total += (size_t)sent;
    }
    free(payload_buf);

    taida_val inner = taida_pack_new(2);
    taida_val ok_hash = taida_str_hash((taida_val)"ok");
    taida_val bytes_hash = taida_str_hash((taida_val)"bytesSent");
    taida_pack_set_hash(inner, 0, ok_hash);
    taida_pack_set(inner, 0, 1);
    taida_pack_set_hash(inner, 1, bytes_hash);
    taida_pack_set(inner, 1, (taida_val)sent_total);

    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_socket_recv(taida_val socket_fd, taida_val timeout_ms) {
    char buf[65536];
    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);
    ssize_t n = recv((int)socket_fd, buf, sizeof(buf) - 1, 0);
    if (n <= 0) {
        const char *kind = (n == 0) ? "peer_closed" : taida_os_error_kind(errno, strerror(errno));
        taida_val error = taida_make_error_with_kind_code("IoError", "SocketRecv error", kind, 0);
        return taida_async_resolved(taida_lax_empty_error((taida_val)"", error));
    }
    buf[n] = '\0';
    char *result = taida_str_new_copy(buf);
    return taida_async_resolved(taida_lax_new((taida_val)result, (taida_val)""));
}

taida_val taida_os_socket_send_bytes(taida_val socket_fd, taida_val data_ptr, taida_val timeout_ms) {
    return taida_os_socket_send(socket_fd, data_ptr, timeout_ms);
}

taida_val taida_os_socket_recv_bytes(taida_val socket_fd, taida_val timeout_ms) {
    unsigned char buf[65536];
    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);
    ssize_t n = recv((int)socket_fd, buf, sizeof(buf), 0);
    if (n <= 0) {
        const char *kind = (n == 0) ? "peer_closed" : taida_os_error_kind(errno, strerror(errno));
        taida_val error = taida_make_error_with_kind_code("IoError", "SocketRecvBytes error", kind, 0);
        return taida_async_resolved(taida_lax_empty_error(taida_bytes_default_value(), error));
    }
    taida_val bytes = taida_bytes_from_raw(buf, (taida_val)n);
    return taida_async_resolved(taida_lax_new(bytes, taida_bytes_default_value()));
}

taida_val taida_os_socket_recv_exact(taida_val socket_fd, taida_val size, taida_val timeout_ms) {
    if (size < 0) {
        taida_val error = taida_make_error_with_kind_code("IoError", "SocketRecvExact error", "invalid", 0);
        return taida_async_resolved(taida_lax_empty_error(taida_bytes_default_value(), error));
    }
    if (size == 0) {
        taida_val empty = taida_bytes_default_value();
        return taida_async_resolved(taida_lax_new(empty, empty));
    }
    // M-11: Cap recv size to 256MB to prevent unbounded malloc from user input.
    if (size > (taida_val)(256 * 1024 * 1024)) {
        taida_val error = taida_make_error_with_kind_code("IoError", "SocketRecvExact error", "too_large", 0);
        return taida_async_resolved(taida_lax_empty_error(taida_bytes_default_value(), error));
    }

    unsigned char *buf = (unsigned char*)malloc((size_t)size);
    if (!buf) {
        taida_val error = taida_make_error_with_kind_code("IoError", "SocketRecvExact error", "other", 0);
        return taida_async_resolved(taida_lax_empty_error(taida_bytes_default_value(), error));
    }

    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);
    size_t total = 0;
    while (total < (size_t)size) {
        ssize_t n = recv((int)socket_fd, buf + total, (size_t)size - total, 0);
        if (n == 0) {
            free(buf);
            taida_val error = taida_make_error_with_kind_code("IoError", "SocketRecvExact error", "peer_closed", 0);
            return taida_async_resolved(taida_lax_empty_error(taida_bytes_default_value(), error));
        }
        if (n < 0) {
            if (errno == EINTR) continue;
            int saved_errno = errno;
            free(buf);
            taida_val error = taida_make_error_with_kind_code(
                "IoError",
                "SocketRecvExact error",
                taida_os_error_kind(saved_errno, strerror(saved_errno)),
                0
            );
            return taida_async_resolved(taida_lax_empty_error(taida_bytes_default_value(), error));
        }
        total += (size_t)n;
    }

    taida_val bytes = taida_bytes_from_raw(buf, size);
    free(buf);
    return taida_async_resolved(taida_lax_new(bytes, taida_bytes_default_value()));
}

static taida_val taida_os_udp_default_payload(void) {
    taida_val payload = taida_pack_new(4);
    taida_val host_hash = taida_str_hash((taida_val)"host");
    taida_val port_hash = taida_str_hash((taida_val)"port");
    taida_val data_hash = taida_str_hash((taida_val)"data");
    taida_val truncated_hash = taida_str_hash((taida_val)"truncated");

    taida_pack_set_hash(payload, 0, host_hash);
    taida_pack_set(payload, 0, (taida_val)taida_str_new_copy(""));
    taida_pack_set_hash(payload, 1, port_hash);
    taida_pack_set(payload, 1, 0);
    taida_pack_set_hash(payload, 2, data_hash);
    taida_pack_set(payload, 2, taida_bytes_default_value());
    taida_pack_set_hash(payload, 3, truncated_hash);
    taida_pack_set(payload, 3, 0);
    return payload;
}

taida_val taida_os_udp_bind(taida_val host_ptr, taida_val port, taida_val timeout_ms) {
    const char *host = (const char*)host_ptr;
    if (!host) return taida_async_resolved(taida_os_result_failure(EINVAL, "udpBind: invalid host"));

    int sockfd = socket(AF_INET, SOCK_DGRAM, 0);
    if (sockfd < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_os_apply_socket_timeout(sockfd, timeout_ms);

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons((unsigned short)port);

    if (host[0] == '\0' || strcmp(host, "0.0.0.0") == 0) {
        addr.sin_addr.s_addr = INADDR_ANY;
    } else if (inet_pton(AF_INET, host, &addr.sin_addr) != 1) {
        // Allow hostnames like "localhost" by resolving via DNS.
        struct addrinfo hints = {0}, *res = NULL;
        hints.ai_family = AF_INET;
        hints.ai_socktype = SOCK_DGRAM;
        int gai = getaddrinfo(host, NULL, &hints, &res);
        if (gai != 0 || !res) {
            close(sockfd);
            return taida_os_dns_failure("udpBind", gai);
        }
        struct sockaddr_in *resolved = (struct sockaddr_in*)res->ai_addr;
        addr.sin_addr = resolved->sin_addr;
        freeaddrinfo(res);
    }

    if (bind(sockfd, (struct sockaddr*)&addr, sizeof(addr)) < 0) {
        close(sockfd);
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_val inner = taida_pack_new(3);
    taida_val socket_hash = taida_str_hash((taida_val)"socket");
    taida_val host_hash = taida_str_hash((taida_val)"host");
    taida_val port_hash = taida_str_hash((taida_val)"port");
    taida_pack_set_hash(inner, 0, socket_hash);
    taida_pack_set(inner, 0, (taida_val)sockfd);
    taida_pack_set_hash(inner, 1, host_hash);
    taida_pack_set(inner, 1, (taida_val)taida_str_new_copy(host));
    taida_pack_set_tag(inner, 1, TAIDA_TAG_STR);
    taida_pack_set_hash(inner, 2, port_hash);
    taida_pack_set(inner, 2, port);

    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_udp_send_to(taida_val socket_fd, taida_val host_ptr, taida_val port, taida_val data_ptr, taida_val timeout_ms) {
    const char *host = (const char*)host_ptr;
    if (!host) {
        return taida_async_resolved(taida_os_result_failure(EINVAL, "udpSendTo: invalid arguments"));
    }

    unsigned char *payload_buf = NULL;
    size_t payload_len = 0;
    if (TAIDA_IS_BYTES(data_ptr)) {
        taida_val *bytes = (taida_val*)data_ptr;
        taida_val len = bytes[1];
        if (len < 0) {
            return taida_async_resolved(taida_os_result_failure(EINVAL, "udpSendTo: invalid bytes payload"));
        }
        // M-15: Cap bytes len to 256MB to prevent unbounded malloc.
        if (len > (taida_val)(256 * 1024 * 1024)) {
            return taida_async_resolved(taida_os_result_failure(EINVAL, "udpSendTo: payload too large"));
        }
        payload_buf = (unsigned char*)TAIDA_MALLOC((size_t)len, "udpSendTo_bytes");
        for (taida_val i = 0; i < len; i++) payload_buf[i] = (unsigned char)bytes[2 + i];
        payload_len = (size_t)len;
    } else {
        const char *data = (const char*)data_ptr;
        size_t data_len = 0;
        if (!taida_read_cstr_len_safe(data, 65536, &data_len)) {
            return taida_async_resolved(taida_os_result_failure(EINVAL, "udpSendTo: invalid data"));
        }
        payload_buf = (unsigned char*)TAIDA_MALLOC(data_len, "socketSend_payload");
        memcpy(payload_buf, data, data_len);
        payload_len = data_len;
    }

    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);

    struct addrinfo hints = {0}, *res = NULL;
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_DGRAM;
    char port_str[16];
    snprintf(port_str, sizeof(port_str), "%" PRId64 "", port);
    int gai = getaddrinfo(host, port_str, &hints, &res);
    if (gai != 0 || !res) {
        free(payload_buf);
        return taida_os_dns_failure("udpSendTo", gai);
    }

    ssize_t sent = sendto((int)socket_fd, payload_buf, payload_len, 0, res->ai_addr, res->ai_addrlen);
    freeaddrinfo(res);
    free(payload_buf);
    if (sent < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }

    taida_val inner = taida_pack_new(2);
    taida_val ok_hash = taida_str_hash((taida_val)"ok");
    taida_val bytes_hash = taida_str_hash((taida_val)"bytesSent");
    taida_pack_set_hash(inner, 0, ok_hash);
    taida_pack_set(inner, 0, 1);
    taida_pack_set_hash(inner, 1, bytes_hash);
    taida_pack_set(inner, 1, sent);
    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_os_udp_recv_from(taida_val socket_fd, taida_val timeout_ms) {
    unsigned char buf[65508];
    struct sockaddr_in from_addr;
    socklen_t from_len = sizeof(from_addr);

    taida_os_apply_socket_timeout((int)socket_fd, timeout_ms);
    ssize_t n = recvfrom((int)socket_fd, buf, sizeof(buf), MSG_TRUNC, (struct sockaddr*)&from_addr, &from_len);
    if (n < 0) {
        taida_val error = taida_make_error_with_kind_code(
            "IoError",
            "UdpRecvFrom error",
            taida_os_error_kind(errno, strerror(errno)),
            0
        );
        return taida_async_resolved(taida_lax_empty_error(taida_os_udp_default_payload(), error));
    }
    taida_val copy_len = (taida_val)n;
    taida_val truncated = 0;
    if ((size_t)n > sizeof(buf)) {
        copy_len = (taida_val)sizeof(buf);
        truncated = 1;
    }

    char host_buf[INET_ADDRSTRLEN] = {0};
    const char *host = inet_ntop(AF_INET, &from_addr.sin_addr, host_buf, sizeof(host_buf));
    if (!host) host = "";
    taida_val peer_port = (taida_val)ntohs(from_addr.sin_port);

    taida_val payload = taida_pack_new(4);
    taida_val host_hash = taida_str_hash((taida_val)"host");
    taida_val port_hash = taida_str_hash((taida_val)"port");
    taida_val data_hash = taida_str_hash((taida_val)"data");
    taida_val truncated_hash = taida_str_hash((taida_val)"truncated");
    taida_pack_set_hash(payload, 0, host_hash);
    taida_pack_set(payload, 0, (taida_val)taida_str_new_copy(host));
    taida_pack_set_tag(payload, 0, TAIDA_TAG_STR);
    taida_pack_set_hash(payload, 1, port_hash);
    taida_pack_set(payload, 1, peer_port);
    taida_pack_set_hash(payload, 2, data_hash);
    taida_pack_set(payload, 2, taida_bytes_from_raw(buf, copy_len));
    taida_pack_set_hash(payload, 3, truncated_hash);
    taida_pack_set(payload, 3, truncated);

    return taida_async_resolved(taida_lax_new(payload, taida_os_udp_default_payload()));
}

taida_val taida_os_socket_close(taida_val socket_fd) {
    if (close((int)socket_fd) < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }
    return taida_async_resolved(taida_os_result_success(taida_os_ok_inner()));
}

taida_val taida_os_listener_close(taida_val listener_fd) {
    if (close((int)listener_fd) < 0) {
        return taida_async_resolved(taida_os_result_failure(errno, strerror(errno)));
    }
    return taida_async_resolved(taida_os_result_success(taida_os_ok_inner()));
}

// ── taida-lang/pool package runtime ──────────────────────

#define TAIDA_POOL_MAX_STATES 4096

typedef struct {
    int open;
    taida_val max_size;
    taida_val max_idle;
    taida_val acquire_timeout_ms;
    taida_val next_token;
    size_t idle_len;
    size_t idle_cap;
    taida_val *idle_tokens;
    taida_val *idle_resources;
    // POOL-4: in-use token membership is an open-addressing hash table
    // (0 = empty, -1 = tombstone; real tokens start at 1) so poolRelease
    // is O(1) instead of a linear scan. Capacity is fixed at creation
    // (first power of two >= 2 * max_size), so no resize path is needed.
    size_t in_use_len;
    size_t in_use_hash_cap;
    taida_val *in_use_hash;
    // Waiting-semaphore state: number of poolAcquire calls currently
    // blocked in pthread_cond_timedwait. Surfaced as poolHealth.waiting.
    taida_val waiting;
} taida_pool_state;

static taida_pool_state *taida_pool_states[TAIDA_POOL_MAX_STATES] = {0};
static taida_val taida_pool_next_id = 1;

// POOL-5: one mutex/cond pair guards the pool table and every pool state.
// httpServe worker threads call pool functions concurrently; the previous
// lock-free table was a data race. Waiters block on the cond and are
// woken by poolRelease / poolClose broadcasts (spurious wakeups are
// absorbed by the re-check loop in taida_pool_acquire).
static pthread_mutex_t taida_pool_mutex = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t taida_pool_cond = PTHREAD_COND_INITIALIZER;

static size_t taida_pool_hash_slot(taida_val token, size_t cap) {
    uint64_t h = (uint64_t)token * 0x9E3779B97F4A7C15ULL;
    return (size_t)(h & (uint64_t)(cap - 1));
}

static int taida_pool_in_use_add(taida_pool_state *st, taida_val token) {
    size_t cap = st->in_use_hash_cap;
    if (cap == 0) return 0;
    size_t i = taida_pool_hash_slot(token, cap);
    for (size_t probe = 0; probe < cap; probe++) {
        taida_val v = st->in_use_hash[i];
        if (v == 0 || v == -1) {
            st->in_use_hash[i] = token;
            st->in_use_len++;
            return 1;
        }
        if (v == token) return 1;
        i = (i + 1) & (cap - 1);
    }
    return 0;
}

static int taida_pool_in_use_remove(taida_pool_state *st, taida_val token) {
    size_t cap = st->in_use_hash_cap;
    if (cap == 0) return 0;
    size_t i = taida_pool_hash_slot(token, cap);
    for (size_t probe = 0; probe < cap; probe++) {
        taida_val v = st->in_use_hash[i];
        if (v == 0) return 0;
        if (v == token) {
            st->in_use_hash[i] = -1;
            st->in_use_len--;
            return 1;
        }
        i = (i + 1) & (cap - 1);
    }
    return 0;
}

static taida_val taida_pool_parse_handle(taida_val pool_or_pack) {
    taida_val pool_hash = taida_str_hash((taida_val)"pool");
    if (taida_is_buchi_pack(pool_or_pack) && taida_pack_has_hash(pool_or_pack, pool_hash)) {
        return taida_pack_get(pool_or_pack, pool_hash);
    }
    return pool_or_pack;
}

static taida_val taida_pool_io_error(const char *kind, const char *msg) {
    const char *message = msg ? msg : "pool error";
    const char *k = kind ? kind : "other";
    taida_val error = taida_pack_new(4);
    taida_pack_set_hash(error, 0, (taida_val)HASH_TYPE);
    taida_pack_set(error, 0, (taida_val)taida_str_new_copy("IoError"));
    taida_pack_set_tag(error, 0, TAIDA_TAG_STR);
    taida_pack_set_hash(error, 1, (taida_val)HASH_MESSAGE);
    taida_pack_set(error, 1, (taida_val)taida_str_new_copy(message));
    taida_pack_set_tag(error, 1, TAIDA_TAG_STR);
    taida_val code_hash = taida_str_hash((taida_val)"code");
    taida_pack_set_hash(error, 2, code_hash);
    taida_pack_set(error, 2, -1);
    taida_val kind_hash = taida_str_hash((taida_val)"kind");
    taida_pack_set_hash(error, 3, kind_hash);
    taida_pack_set(error, 3, (taida_val)taida_str_new_copy(k));
    taida_pack_set_tag(error, 3, TAIDA_TAG_STR);
    return error;
}

static taida_val taida_pool_result_failure(const char *kind, const char *msg) {
    const char *message = msg ? msg : "pool error";
    const char *k = kind ? kind : "other";
    taida_val inner = taida_pack_new(4);
    taida_val ok_hash = taida_str_hash((taida_val)"ok");
    taida_val code_hash = taida_str_hash((taida_val)"code");
    taida_val msg_hash = taida_str_hash((taida_val)"message");
    taida_val kind_hash = taida_str_hash((taida_val)"kind");
    taida_pack_set_hash(inner, 0, ok_hash);
    taida_pack_set(inner, 0, 0);
    taida_pack_set_hash(inner, 1, code_hash);
    taida_pack_set(inner, 1, -1);
    taida_pack_set_hash(inner, 2, msg_hash);
    taida_pack_set(inner, 2, (taida_val)taida_str_new_copy(message));
    taida_pack_set_tag(inner, 2, TAIDA_TAG_STR);
    taida_pack_set_hash(inner, 3, kind_hash);
    taida_pack_set(inner, 3, (taida_val)taida_str_new_copy(k));
    taida_pack_set_tag(inner, 3, TAIDA_TAG_STR);
    return taida_result_create(inner, taida_pool_io_error(k, message), 0);
}

static int taida_pool_push_idle(taida_pool_state *st, taida_val token, taida_val resource) {
    if (st->idle_len >= st->idle_cap) {
        size_t new_cap = st->idle_cap == 0 ? 4 : st->idle_cap * 2;
        taida_val *new_tokens = (taida_val*)realloc(st->idle_tokens, sizeof(taida_val) * new_cap);
        taida_val *new_resources = (taida_val*)realloc(st->idle_resources, sizeof(taida_val) * new_cap);
        if (!new_tokens || !new_resources) {
            if (new_tokens) st->idle_tokens = new_tokens;
            if (new_resources) st->idle_resources = new_resources;
            return 0;
        }
        st->idle_tokens = new_tokens;
        st->idle_resources = new_resources;
        st->idle_cap = new_cap;
    }
    st->idle_tokens[st->idle_len] = token;
    st->idle_resources[st->idle_len] = resource;
    st->idle_len++;
    return 1;
}

static taida_val taida_pool_health_pack(taida_val open, taida_val idle, taida_val in_use, taida_val waiting) {
    taida_val pack = taida_pack_new(4);
    taida_val open_hash = taida_str_hash((taida_val)"open");
    taida_val idle_hash = taida_str_hash((taida_val)"idle");
    taida_val in_use_hash = taida_str_hash((taida_val)"inUse");
    taida_val waiting_hash = taida_str_hash((taida_val)"waiting");
    taida_pack_set_hash(pack, 0, open_hash);
    taida_pack_set(pack, 0, open ? 1 : 0);
    taida_pack_set_hash(pack, 1, idle_hash);
    taida_pack_set(pack, 1, idle);
    taida_pack_set_hash(pack, 2, in_use_hash);
    taida_pack_set(pack, 2, in_use);
    taida_pack_set_hash(pack, 3, waiting_hash);
    taida_pack_set(pack, 3, waiting);
    return pack;
}

taida_val taida_pool_create(taida_val config_ptr) {
    if (!taida_is_buchi_pack(config_ptr)) {
        return taida_pool_result_failure("invalid", "poolCreate: config must be a pack");
    }

    taida_val max_size = 10;
    taida_val max_idle = 10;
    taida_val acquire_timeout_ms = 30000;
    taida_val max_size_hash = taida_str_hash((taida_val)"maxSize");
    taida_val max_idle_hash = taida_str_hash((taida_val)"maxIdle");
    taida_val timeout_hash = taida_str_hash((taida_val)"acquireTimeoutMs");

    if (taida_pack_has_hash(config_ptr, max_size_hash)) {
        max_size = taida_pack_get(config_ptr, max_size_hash);
    }
    if (taida_pack_has_hash(config_ptr, max_idle_hash)) {
        max_idle = taida_pack_get(config_ptr, max_idle_hash);
    }
    if (taida_pack_has_hash(config_ptr, timeout_hash)) {
        acquire_timeout_ms = taida_pack_get(config_ptr, timeout_hash);
    }

    if (max_size <= 0) {
        return taida_pool_result_failure("invalid", "poolCreate: maxSize must be > 0");
    }
    if (max_idle < 0) {
        return taida_pool_result_failure("invalid", "poolCreate: maxIdle must be >= 0");
    }
    if (max_idle > max_size) max_idle = max_size;
    if (acquire_timeout_ms <= 0) {
        return taida_pool_result_failure("invalid", "poolCreate: acquireTimeoutMs must be > 0");
    }

    pthread_mutex_lock(&taida_pool_mutex);
    taida_val pool_id = taida_pool_next_id++;
    if (pool_id <= 0 || pool_id >= TAIDA_POOL_MAX_STATES) {
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_pool_result_failure("other", "poolCreate: pool table exhausted");
    }

    taida_pool_state *st = (taida_pool_state*)calloc(1, sizeof(taida_pool_state));
    if (!st) {
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_pool_result_failure("other", "poolCreate: out of memory");
    }
    st->open = 1;
    st->max_size = max_size;
    st->max_idle = max_idle;
    st->acquire_timeout_ms = acquire_timeout_ms;
    st->next_token = 1;
    // POOL-4: fixed-capacity in-use hash — first power of two >= 2*max_size.
    size_t hash_cap = 4;
    while ((taida_val)hash_cap < max_size * 2) hash_cap <<= 1;
    st->in_use_hash = (taida_val*)calloc(hash_cap, sizeof(taida_val));
    if (!st->in_use_hash) {
        free(st);
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_pool_result_failure("other", "poolCreate: out of memory");
    }
    st->in_use_hash_cap = hash_cap;
    taida_pool_states[pool_id] = st;
    pthread_mutex_unlock(&taida_pool_mutex);

    taida_val inner = taida_pack_new(1);
    taida_val pool_hash = taida_str_hash((taida_val)"pool");
    taida_pack_set_hash(inner, 0, pool_hash);
    taida_pack_set(inner, 0, pool_id);
    return taida_os_result_success(inner);
}

// Try to take a slot while holding taida_pool_mutex. On success the
// Lax-wrapped resource + token are stored and the token is registered
// as in-use. Returns 1 acquired / 0 exhausted / -1 out of memory.
static int taida_pool_try_take_slot(taida_pool_state *st, taida_val *out_lax, taida_val *out_token) {
    taida_val token = 0;
    taida_val resource_lax = 0;
    if (st->idle_len > 0) {
        st->idle_len--;
        token = st->idle_tokens[st->idle_len];
        resource_lax = taida_lax_new(st->idle_resources[st->idle_len], 0);
    } else if ((taida_val)st->in_use_len < st->max_size) {
        token = st->next_token++;
        resource_lax = taida_lax_empty(0);
    } else {
        return 0;
    }
    if (!taida_pool_in_use_add(st, token)) return -1;
    *out_lax = resource_lax;
    *out_token = token;
    return 1;
}

// Build the success payload for an acquired slot: Result[@(resource, token)].
static taida_val taida_pool_acquire_success(taida_val resource_lax, taida_val token) {
    taida_val inner = taida_pack_new(2);
    taida_val resource_hash = taida_str_hash((taida_val)"resource");
    taida_val token_hash = taida_str_hash((taida_val)"token");
    taida_pack_set_hash(inner, 0, resource_hash);
    taida_pack_set(inner, 0, resource_lax);
    taida_pack_set_tag(inner, 0, TAIDA_TAG_PACK);
    taida_pack_set_hash(inner, 1, token_hash);
    taida_pack_set(inner, 1, token);
    return taida_os_result_success(inner);
}

// F54B-024: a pool-exhausted acquire must stay pending until awaited.
// The cond-wait loop runs on this background pthread, which resolves the
// pending Async with the same write protocol as taida_thread_entry
// (value + tag, release fence, then fulfilled status). The caller gets
// the pending Async back immediately, so it can poolRelease from the
// same Taida thread before awaiting — interpreter (tokio task) and JS
// (async function) both keep the exhausted acquire pending this way.
typedef struct {
    taida_val pool_id;
    taida_val effective_timeout;
    struct timespec deadline;
    taida_val *async_obj;
} taida_pool_wait_arg;

static void* taida_pool_acquire_wait_thread(void *raw) {
    taida_pool_wait_arg *wa = (taida_pool_wait_arg*)raw;
    taida_val result = 0;
    pthread_mutex_lock(&taida_pool_mutex);
    // Pool states are never freed (poolClose only flips `open`), so the
    // entry fetched here stays valid for the whole wait. The caller
    // already registered this waiter (st->waiting++).
    taida_pool_state *st = taida_pool_states[wa->pool_id];
    for (;;) {
        if (!st || !st->open) {
            if (st) st->waiting--;
            pthread_mutex_unlock(&taida_pool_mutex);
            result = taida_pool_result_failure("closed", "poolAcquire: pool is closed");
            break;
        }
        taida_val resource_lax = 0;
        taida_val token = 0;
        int took = taida_pool_try_take_slot(st, &resource_lax, &token);
        if (took != 0) {
            st->waiting--;
            pthread_mutex_unlock(&taida_pool_mutex);
            result = (took < 0)
                ? taida_pool_result_failure("other", "poolAcquire: out of memory")
                : taida_pool_acquire_success(resource_lax, token);
            break;
        }
        int rc = pthread_cond_timedwait(&taida_pool_cond, &taida_pool_mutex, &wa->deadline);
        if (rc == ETIMEDOUT) {
            st->waiting--;
            pthread_mutex_unlock(&taida_pool_mutex);
            char msg[96];
            snprintf(msg, sizeof(msg), "poolAcquire: timed out after %" PRId64 "ms", wa->effective_timeout);
            result = taida_pool_result_failure("timeout", msg);
            break;
        }
        // Broadcast or spurious wakeup: loop and re-check the pool state.
    }
    wa->async_obj[2] = result;
    wa->async_obj[5] = taida_detect_value_tag(result);
    __atomic_thread_fence(__ATOMIC_RELEASE);  // barrier: value+tag visible before status
    wa->async_obj[1] = 1;  // mark fulfilled (must be last — signals to readers)
    free(wa);
    return NULL;
}

taida_val taida_pool_acquire(taida_val pool_or_pack, taida_val timeout_ms) {
    taida_val pool_id = taida_pool_parse_handle(pool_or_pack);
    pthread_mutex_lock(&taida_pool_mutex);
    if (pool_id <= 0 || pool_id >= TAIDA_POOL_MAX_STATES || !taida_pool_states[pool_id]) {
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_async_resolved(taida_pool_result_failure("invalid", "poolAcquire: unknown pool handle"));
    }

    taida_pool_state *st = taida_pool_states[pool_id];
    if (!st->open) {
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_async_resolved(taida_pool_result_failure("closed", "poolAcquire: pool is closed"));
    }

    // INT64_MIN is the omitted-argument sentinel injected by the lowering:
    // fall back to the pool's configured acquireTimeoutMs (interpreter
    // parity). Any other non-positive explicit value is invalid.
    taida_val effective_timeout;
    if (timeout_ms == INT64_MIN) {
        effective_timeout = st->acquire_timeout_ms;
    } else if (timeout_ms <= 0) {
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_async_resolved(taida_pool_result_failure("invalid", "poolAcquire: timeoutMs must be > 0"));
    } else {
        effective_timeout = timeout_ms;
    }

    // Waiting-semaphore contract: the acquired pack carries
    // `resource: Lax` — Lax(value) when an idle (BYO, deposited via
    // poolRelease) resource is reused, failure-side Lax (placeholder
    // default 0; the pool does not know the element type) when the slot
    // is fresh. Fast path: a slot is free right now — resolve inline.
    taida_val resource_lax = 0;
    taida_val token = 0;
    int took = taida_pool_try_take_slot(st, &resource_lax, &token);
    if (took < 0) {
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_async_resolved(taida_pool_result_failure("other", "poolAcquire: out of memory"));
    }
    if (took > 0) {
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_async_resolved(taida_pool_acquire_success(resource_lax, token));
    }

    // Pool exhausted (F54B-024): blocking here would deadlock the
    // "create a pending acquire, release, then await" pattern — the
    // releasing code runs on this very thread. Register as a waiter NOW
    // (poolHealth.waiting must reflect the pending acquire immediately,
    // interpreter parity), then hand the cond-wait loop to a background
    // pthread and return a pending Async; `>=>` joins it on await.
    st->waiting++;
    pthread_mutex_unlock(&taida_pool_mutex);

    struct timespec deadline;
    clock_gettime(CLOCK_REALTIME, &deadline);
    deadline.tv_sec += effective_timeout / 1000;
    deadline.tv_nsec += (long)(effective_timeout % 1000) * 1000000L;
    if (deadline.tv_nsec >= 1000000000L) {
        deadline.tv_sec += 1;
        deadline.tv_nsec -= 1000000000L;
    }

    taida_pool_wait_arg *wa = (taida_pool_wait_arg*)malloc(sizeof(taida_pool_wait_arg));
    taida_val *obj = (taida_val*)malloc(7 * sizeof(taida_val));
    pthread_t waiter;
    int spawn_ok = 0;
    if (wa && obj) {
        obj[0] = TAIDA_ASYNC_MAGIC | 1;  // magic + refcount
        obj[1] = 0;                      // status: pending
        obj[2] = 0;                      // no value yet
        obj[3] = 0;                      // no error
        obj[4] = 0;                      // thread handle (set below)
        obj[5] = TAIDA_TAG_UNKNOWN;      // value_tag (set on resolve)
        obj[6] = TAIDA_TAG_UNKNOWN;      // error_tag (unused)
        wa->pool_id = pool_id;
        wa->effective_timeout = effective_timeout;
        wa->deadline = deadline;
        wa->async_obj = obj;
        spawn_ok = pthread_create(&waiter, NULL, taida_pool_acquire_wait_thread, wa) == 0;
    }
    if (!spawn_ok) {
        free(wa);
        free(obj);
        pthread_mutex_lock(&taida_pool_mutex);
        if (taida_pool_states[pool_id]) taida_pool_states[pool_id]->waiting--;
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_async_resolved(taida_pool_result_failure("other", "poolAcquire: cannot start waiter thread"));
    }
    obj[4] = (taida_val)waiter;
    return (taida_val)obj;
}

taida_val taida_pool_release(taida_val pool_or_pack, taida_val token, taida_val resource) {
    taida_val pool_id = taida_pool_parse_handle(pool_or_pack);
    pthread_mutex_lock(&taida_pool_mutex);
    if (pool_id <= 0 || pool_id >= TAIDA_POOL_MAX_STATES || !taida_pool_states[pool_id]) {
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_pool_result_failure("invalid", "poolRelease: unknown pool handle");
    }

    taida_pool_state *st = taida_pool_states[pool_id];
    if (!st->open) {
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_pool_result_failure("closed", "poolRelease: pool is closed");
    }

    if (!taida_pool_in_use_remove(st, token)) {
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_pool_result_failure("invalid", "poolRelease: token is not in-use");
    }

    taida_val reused = 0;
    if ((taida_val)st->idle_len < st->max_idle) {
        if (!taida_pool_push_idle(st, token, resource)) {
            pthread_mutex_unlock(&taida_pool_mutex);
            return taida_pool_result_failure("other", "poolRelease: out of memory");
        }
        reused = 1;
    }
    // A slot just freed up — wake every blocked poolAcquire so one of
    // them can take it (the rest re-check and keep waiting).
    pthread_cond_broadcast(&taida_pool_cond);
    pthread_mutex_unlock(&taida_pool_mutex);

    taida_val inner = taida_pack_new(2);
    taida_val ok_hash = taida_str_hash((taida_val)"ok");
    taida_val reused_hash = taida_str_hash((taida_val)"reused");
    taida_pack_set_hash(inner, 0, ok_hash);
    taida_pack_set(inner, 0, 1);
    taida_pack_set_hash(inner, 1, reused_hash);
    taida_pack_set(inner, 1, reused);
    return taida_os_result_success(inner);
}

taida_val taida_pool_close(taida_val pool_or_pack) {
    taida_val pool_id = taida_pool_parse_handle(pool_or_pack);
    pthread_mutex_lock(&taida_pool_mutex);
    if (pool_id <= 0 || pool_id >= TAIDA_POOL_MAX_STATES || !taida_pool_states[pool_id]) {
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_async_resolved(taida_pool_result_failure("invalid", "poolClose: unknown pool handle"));
    }
    taida_pool_state *st = taida_pool_states[pool_id];
    if (!st->open) {
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_async_resolved(taida_pool_result_failure("closed", "poolClose: pool already closed"));
    }
    st->open = 0;
    st->idle_len = 0;
    st->in_use_len = 0;
    if (st->in_use_hash) {
        memset(st->in_use_hash, 0, st->in_use_hash_cap * sizeof(taida_val));
    }
    // Wake every blocked poolAcquire so they fail with kind:"closed".
    pthread_cond_broadcast(&taida_pool_cond);
    pthread_mutex_unlock(&taida_pool_mutex);

    taida_val inner = taida_pack_new(1);
    taida_val ok_hash = taida_str_hash((taida_val)"ok");
    taida_pack_set_hash(inner, 0, ok_hash);
    taida_pack_set(inner, 0, 1);
    return taida_async_resolved(taida_os_result_success(inner));
}

taida_val taida_pool_health(taida_val pool_or_pack) {
    taida_val pool_id = taida_pool_parse_handle(pool_or_pack);
    pthread_mutex_lock(&taida_pool_mutex);
    if (pool_id <= 0 || pool_id >= TAIDA_POOL_MAX_STATES || !taida_pool_states[pool_id]) {
        pthread_mutex_unlock(&taida_pool_mutex);
        return taida_pool_health_pack(0, 0, 0, 0);
    }
    taida_pool_state *st = taida_pool_states[pool_id];
    taida_val open = st->open;
    taida_val idle = (taida_val)st->idle_len;
    taida_val in_use = (taida_val)st->in_use_len;
    taida_val waiting = st->waiting;
    pthread_mutex_unlock(&taida_pool_mutex);
    return taida_pool_health_pack(open, idle, in_use, waiting);
}
