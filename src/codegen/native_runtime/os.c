// ── taida-lang/os package — Native runtime ────────────────

// Helper: build os Result pack. OS Result constructors preserve the
// interpreter's field order: __value, throw, __predicate, __type.
static taida_val taida_os_result_create(taida_val inner, taida_val throw_val) {
    taida_register_result_field_names();
    taida_val pack = taida_pack_new(4);
    taida_pack_set_hash(pack, 0, (taida_val)HASH_RES___VALUE);
    taida_pack_set(pack, 0, inner);
    taida_retain_and_tag_field(pack, 0, inner);
    taida_pack_set_hash(pack, 1, (taida_val)HASH_RES_THROW);
    taida_pack_set(pack, 1, throw_val);
    if (throw_val != 0) {
        taida_retain_and_tag_field(pack, 1, throw_val);
    } else {
        taida_pack_set_tag(pack, 1, TAIDA_TAG_PACK);
    }
    taida_pack_set_hash(pack, 2, (taida_val)HASH_RES___PREDICATE);
    taida_pack_set(pack, 2, 0);
    taida_pack_set_tag(pack, 2, TAIDA_TAG_PACK);
    taida_pack_set_hash(pack, 3, (taida_val)HASH___TYPE);
    taida_pack_set(pack, 3, (taida_val)__result_type_str);
    taida_pack_set_tag(pack, 3, TAIDA_TAG_STR);
    return pack;
}

// Helper: build os Result success BuchiPack.
static taida_val taida_os_result_success(taida_val inner) {
    return taida_os_result_create(inner, 0);
}

// Helper: build os Result failure with IoError
static taida_val taida_os_result_failure(int err_code, const char *err_msg) {
    // inner = @(ok=false, code=errno, message=err_msg, kind=...)
    const char *message = err_msg ? err_msg : "unknown io error";
    const char *kind = taida_os_error_kind(err_code, message);
    taida_val inner = taida_pack_new(4);
    // ok field
    taida_val ok_hash = 0x08b05d07b5566befULL;  // FNV-1a("ok")
    taida_pack_set_hash(inner, 0, (taida_val)ok_hash);
    taida_pack_set(inner, 0, 0);  // false
    // code field
    taida_val code_hash = 0x0bb51791194b4414ULL;  // FNV-1a("code")
    taida_pack_set_hash(inner, 1, (taida_val)code_hash);
    taida_pack_set(inner, 1, (taida_val)err_code);
    // message field
    taida_val msg_hash = 0x546401b5d2a8d2a4ULL;   // FNV-1a("message")
    taida_pack_set_hash(inner, 2, (taida_val)msg_hash);
    char *msg_copy = taida_str_new_copy(message);
    taida_pack_set(inner, 2, (taida_val)msg_copy);
    // kind field
    taida_val kind_hash = taida_str_hash((taida_val)"kind");
    taida_pack_set_hash(inner, 3, kind_hash);
    char *kind_copy = taida_str_new_copy(kind);
    taida_pack_set(inner, 3, (taida_val)kind_copy);

    taida_val error = taida_make_io_error(err_code, message);
    return taida_os_result_create(inner, error);
}

// Helper: build os ok inner @(ok=true, code=0, message="")
static taida_val taida_os_ok_inner(void) {
    taida_val inner = taida_pack_new(3);
    taida_val ok_hash = 0x08b05d07b5566befULL;
    taida_pack_set_hash(inner, 0, (taida_val)ok_hash);
    taida_pack_set(inner, 0, 1);  // true
    taida_val code_hash = 0x0bb51791194b4414ULL;
    taida_pack_set_hash(inner, 1, (taida_val)code_hash);
    taida_pack_set(inner, 1, 0);
    taida_val msg_hash = 0x546401b5d2a8d2a4ULL;
    taida_pack_set_hash(inner, 2, (taida_val)msg_hash);
    taida_pack_set(inner, 2, (taida_val)"");
    return inner;
}

// Helper: build process result inner @(stdout, stderr, code)
static taida_val taida_os_process_inner(const char *out, const char *err, taida_val code) {
    taida_val inner = taida_pack_new(3);
    // stdout
    taida_val stdout_hash = 0x42e6d785a74f8c66ULL;  // FNV-1a("stdout")
    taida_pack_set_hash(inner, 0, (taida_val)stdout_hash);
    char *out_copy = taida_str_new_copy(out);
    taida_pack_set(inner, 0, (taida_val)out_copy);
    // stderr
    taida_val stderr_hash = 0x104ce5858b0a80b5ULL;  // FNV-1a("stderr")
    taida_pack_set_hash(inner, 1, (taida_val)stderr_hash);
    char *err_copy = taida_str_new_copy(err);
    taida_pack_set(inner, 1, (taida_val)err_copy);
    // code
    taida_val code_hash = 0x0bb51791194b4414ULL;
    taida_pack_set_hash(inner, 2, (taida_val)code_hash);
    taida_pack_set(inner, 2, code);
    return inner;
}

// C19: build code-only process result inner @(code: Int).
// Used by runInteractive / execShellInteractive. stdout / stderr are not
// captured because the child inherits the TTY directly.
static taida_val taida_os_process_inner_code_only(taida_val code) {
    taida_val inner = taida_pack_new(1);
    taida_val code_hash = 0x0bb51791194b4414ULL;  // FNV-1a("code")
    taida_pack_set_hash(inner, 0, (taida_val)code_hash);
    taida_pack_set(inner, 0, code);
    return inner;
}

// C19: derive exit code from a `waitpid` status, following the
// `128 + signum` POSIX convention used by the interpreter / JS backends.
static taida_val taida_os_extract_wait_code(int status) {
    if (WIFEXITED(status)) return (taida_val)WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return (taida_val)(128 + WTERMSIG(status));
    return (taida_val)(-1);
}

// ── Read[path]() → Lax[Str] ──────────────────────────────
static taida_val taida_os_read_lax_error(const char *kind) {
    taida_val error = taida_make_error_with_kind_code("IoError", "Read error", kind, 0);
    return taida_lax_empty_error((taida_val)"", error);
}

#include <poll.h>  // G3: poll() for concurrent stdout/stderr drain

// G3 (NET/OS): concurrently drain a child's stdout and stderr pipes. Draining
// one pipe to EOF before touching the other deadlocks when the child writes
// more than a pipe buffer (~64KB) to the undrained pipe: the child blocks in
// write() while the parent blocks in read() of the other pipe. Set both fds
// non-blocking and poll() them, reading whichever is ready, until both reach
// EOF. Both output buffers are heap-allocated (caller frees) and NUL-terminated.
static void taida_os_drain_two_pipes(int out_fd, int err_fd,
                                     char **out_buf_p, size_t *out_len_p,
                                     char **err_buf_p, size_t *err_len_p) {
    int of = fcntl(out_fd, F_GETFL, 0);
    if (of != -1) fcntl(out_fd, F_SETFL, of | O_NONBLOCK);
    int ef = fcntl(err_fd, F_GETFL, 0);
    if (ef != -1) fcntl(err_fd, F_SETFL, ef | O_NONBLOCK);

    char *bufs[2];
    size_t caps[2] = { 4096, 4096 };
    size_t lens[2] = { 0, 0 };
    bufs[0] = (char*)TAIDA_MALLOC(caps[0], "os_drain_stdout");
    bufs[1] = (char*)TAIDA_MALLOC(caps[1], "os_drain_stderr");

    struct pollfd fds[2];
    fds[0].fd = out_fd; fds[0].events = POLLIN; fds[0].revents = 0;
    fds[1].fd = err_fd; fds[1].events = POLLIN; fds[1].revents = 0;
    int open_count = 2;

    while (open_count > 0) {
        int pr = poll(fds, 2, -1);
        if (pr < 0) {
            if (errno == EINTR) continue;
            break; // unexpected poll error — return what we have so far
        }
        for (int k = 0; k < 2; k++) {
            if (fds[k].fd < 0) continue;
            if (!(fds[k].revents & (POLLIN | POLLHUP | POLLERR))) continue;
            // Drain everything currently buffered on this fd.
            for (;;) {
                if (lens[k] + 1 >= caps[k]) {
                    caps[k] *= 2;
                    TAIDA_REALLOC(bufs[k], caps[k], "os_drain");
                }
                ssize_t n = read(fds[k].fd, bufs[k] + lens[k], caps[k] - lens[k] - 1);
                if (n > 0) {
                    lens[k] += (size_t)n;
                } else if (n == 0) {
                    close(fds[k].fd);
                    fds[k].fd = -1;
                    open_count--;
                    break;
                } else {
                    if (errno == EAGAIN || errno == EWOULDBLOCK) break;
                    if (errno == EINTR) continue;
                    close(fds[k].fd);
                    fds[k].fd = -1;
                    open_count--;
                    break;
                }
            }
        }
    }

    bufs[0][lens[0]] = '\0';
    bufs[1][lens[1]] = '\0';
    *out_buf_p = bufs[0]; *out_len_p = lens[0];
    *err_buf_p = bufs[1]; *err_len_p = lens[1];
}

taida_val taida_os_read(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_os_read_lax_error("invalid");

    // Check file size (64MB limit)
    struct stat st;
    if (stat(path, &st) != 0) return taida_os_read_lax_error(taida_os_error_kind(errno, strerror(errno)));
    if (st.st_size > 64 * 1024 * 1024) return taida_os_read_lax_error("too_large");

    FILE *f = fopen(path, "r");
    if (!f) return taida_os_read_lax_error(taida_os_error_kind(errno, strerror(errno)));

    taida_val size = st.st_size;
    char *buf = taida_str_alloc(size);
    taida_val read_bytes = (taida_val)fread(buf, 1, size, f);
    fclose(f);
    buf[read_bytes] = '\0';

    return taida_lax_new((taida_val)buf, (taida_val)"");
}

// ── readBytes(path) → Lax[Bytes] ──────────────────────────
static taida_val taida_os_read_bytes_lax_error(const char *kind) {
    taida_val error = taida_make_error_with_kind_code("IoError", "ReadBytes error", kind, 0);
    return taida_lax_empty_error(taida_bytes_default_value(), error);
}

taida_val taida_os_read_bytes(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_os_read_bytes_lax_error("invalid");

    struct stat st;
    if (stat(path, &st) != 0) return taida_os_read_bytes_lax_error(taida_os_error_kind(errno, strerror(errno)));
    if (st.st_size > 64 * 1024 * 1024) return taida_os_read_bytes_lax_error("too_large");

    FILE *f = fopen(path, "rb");
    if (!f) return taida_os_read_bytes_lax_error(taida_os_error_kind(errno, strerror(errno)));

    taida_val size = st.st_size;
    unsigned char *buf = NULL;
    if (size > 0) {
        buf = (unsigned char*)malloc((size_t)size);
        if (!buf) {
            fclose(f);
            return taida_os_read_bytes_lax_error("other");
        }
    }

    size_t read_bytes = 0;
    if (size > 0) {
        read_bytes = fread(buf, 1, (size_t)size, f);
    }
    fclose(f);

    taida_val bytes = taida_bytes_from_raw(buf, (taida_val)read_bytes);
    free(buf);
    return taida_lax_new(bytes, taida_bytes_default_value());
}

// ── C26B-020 柱 1: readBytesAt(path, offset, len) → Lax[Bytes] ──
//
// Chunked file read. Semantics mirror the interpreter:
//   - offset < 0 or len < 0       → Lax failure (default empty Bytes)
//   - len == 0                    → Lax success (empty Bytes)
//   - len > 64 MB chunk ceiling   → Lax failure
//   - offset >= file size         → Lax success (empty Bytes, short read)
//   - offset + len > file size    → Lax success (truncated tail)
//   - IO error                    → Lax failure
static taida_val taida_os_read_bytes_at_lax_error(const char *kind) {
    taida_val error = taida_make_error_with_kind_code("IoError", "ReadBytesAt error", kind, 0);
    return taida_lax_empty_error(taida_bytes_default_value(), error);
}

taida_val taida_os_read_bytes_at(taida_val path_ptr, taida_val offset, taida_val len) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_os_read_bytes_at_lax_error("invalid");
    if (offset < 0 || len < 0) return taida_os_read_bytes_at_lax_error("invalid");
    if (len > 64 * 1024 * 1024) return taida_os_read_bytes_at_lax_error("too_large");
    if (len == 0) {
        taida_val empty = taida_bytes_from_raw(NULL, 0);
        return taida_lax_new(empty, taida_bytes_default_value());
    }

    FILE *f = fopen(path, "rb");
    if (!f) return taida_os_read_bytes_at_lax_error(taida_os_error_kind(errno, strerror(errno)));

    if (fseeko(f, (off_t)offset, SEEK_SET) != 0) {
        int saved_errno = errno;
        fclose(f);
        return taida_os_read_bytes_at_lax_error(taida_os_error_kind(saved_errno, strerror(saved_errno)));
    }

    unsigned char *buf = (unsigned char*)malloc((size_t)len);
    if (!buf) {
        fclose(f);
        return taida_os_read_bytes_at_lax_error("other");
    }

    // Tolerate short reads at EOF: loop fread until full or EOF.
    size_t filled = 0;
    while (filled < (size_t)len) {
        size_t got = fread(buf + filled, 1, (size_t)len - filled, f);
        if (got == 0) break;
        filled += got;
    }
    int io_err = ferror(f);
    fclose(f);

    if (io_err) {
        free(buf);
        return taida_os_read_bytes_at_lax_error("other");
    }

    taida_val bytes = taida_bytes_from_raw(buf, (taida_val)filled);
    free(buf);
    return taida_lax_new(bytes, taida_bytes_default_value());
}

// ── String comparator for qsort ──────────────────────────
static int taida_cmp_strings(const void *a, const void *b) {
    return strcmp(*(const char**)a, *(const char**)b);
}

static taida_val taida_os_list_dir_lax_error(const char *kind) {
    taida_val error = taida_make_error_with_kind_code("IoError", "ListDir error", kind, 0);
    return taida_lax_empty_error(taida_list_new(), error);
}

// ── ListDir[path]() → Lax[@[Str]] ────────────────────────
taida_val taida_os_list_dir(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_os_list_dir_lax_error("invalid");

    DIR *dir = opendir(path);
    if (!dir) return taida_os_list_dir_lax_error(taida_os_error_kind(errno, strerror(errno)));

    // Collect entries, then sort
    taida_val capacity = 64;
    taida_val count = 0;
    char **names = (char**)TAIDA_MALLOC(capacity * sizeof(char*), "listDir_init");

    struct dirent *entry;
    while ((entry = readdir(dir)) != NULL) {
        // Skip . and ..
        if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) continue;
        if (count >= capacity) {
            // M-12: Guard against taida_val overflow on capacity *= 2.
            // capacity is int64_t; if it exceeds INT64_MAX/2, doubling would
            // overflow. In practice this is unreachable (>4 billion entries),
            // but the guard prevents undefined behavior.
            if (capacity > (taida_val)(INT64_MAX / 2)) {
                fprintf(stderr, "taida: directory entry count overflow in taida_os_list_dir\n");
                // Clean up already-collected names
                for (taida_val i = 0; i < count; i++) taida_str_release((taida_val)names[i]);
                free(names);
                closedir(dir);
                return taida_os_list_dir_lax_error("too_large");
            }
            capacity *= 2;
            TAIDA_REALLOC(names, taida_safe_mul((size_t)capacity, sizeof(char*), "listDir_grow"), "listDir");
        }
        names[count] = taida_str_new_copy(entry->d_name);
        count++;
    }
    closedir(dir);

    // Sort alphabetically
    if (count > 1) {
        qsort(names, count, sizeof(char*), taida_cmp_strings);
    }

    taida_val list = taida_list_new();
    for (taida_val i = 0; i < count; i++) {
        list = taida_list_push(list, (taida_val)names[i]);
    }
    free(names);

    return taida_lax_new(list, taida_list_new());
}

static taida_val taida_os_stat_default_pack(void) {
    taida_val default_pack = taida_pack_new(3);
    taida_val size_hash = 0x4dea9618e618ae3cULL;     // FNV-1a("size")
    taida_val modified_hash = 0xd381b19c7fd35852ULL;  // FNV-1a("modified")
    taida_val is_dir_hash = 0x641d9cfa1a584ee4ULL;    // FNV-1a("isDir")
    taida_pack_set_hash(default_pack, 0, (taida_val)size_hash);
    taida_pack_set(default_pack, 0, 0);
    taida_pack_set_hash(default_pack, 1, (taida_val)modified_hash);
    taida_pack_set(default_pack, 1, (taida_val)"");
    taida_pack_set_hash(default_pack, 2, (taida_val)is_dir_hash);
    taida_pack_set(default_pack, 2, 0);
    return default_pack;
}

static taida_val taida_os_stat_lax_error(const char *kind) {
    taida_val error = taida_make_error_with_kind_code("IoError", "Stat error", kind, 0);
    return taida_lax_empty_error(taida_os_stat_default_pack(), error);
}

// ── Stat[path]() → Lax[@(size: Int, modified: Str, isDir: Bool)] ──
taida_val taida_os_stat(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;

    if (!path) return taida_os_stat_lax_error("invalid");

    struct stat st;
    if (stat(path, &st) != 0) return taida_os_stat_lax_error(taida_os_error_kind(errno, strerror(errno)));

    taida_val size_hash = 0x4dea9618e618ae3cULL;     // FNV-1a("size")
    taida_val modified_hash = 0xd381b19c7fd35852ULL;  // FNV-1a("modified")
    taida_val is_dir_hash = 0x641d9cfa1a584ee4ULL;    // FNV-1a("isDir")
    taida_val default_pack = taida_os_stat_default_pack();

    // Format modified time as RFC3339/UTC
    struct tm tm_buf;
    struct tm *tm_utc = gmtime_r(&st.st_mtime, &tm_buf);
    char time_buf[32];
    if (tm_utc) {
        strftime(time_buf, sizeof(time_buf), "%Y-%m-%dT%H:%M:%SZ", tm_utc);
    } else {
        // R-11: memcpy for fixed-length literal (no format parsing overhead)
        memcpy(time_buf, "1970-01-01T00:00:00Z", 21); /* 20 chars + '\0' */
    }
    char *time_str = taida_str_new_copy(time_buf);

    taida_val stat_pack = taida_pack_new(3);
    taida_pack_set_hash(stat_pack, 0, (taida_val)size_hash);
    taida_pack_set(stat_pack, 0, (taida_val)st.st_size);
    taida_pack_set_hash(stat_pack, 1, (taida_val)modified_hash);
    taida_pack_set(stat_pack, 1, (taida_val)time_str);
    taida_pack_set_hash(stat_pack, 2, (taida_val)is_dir_hash);
    taida_pack_set(stat_pack, 2, S_ISDIR(st.st_mode) ? 1 : 0);

    return taida_lax_new(stat_pack, default_pack);
}

// ── Exists[path]() → Result[Bool] ─────────────────────────
//
// C12B-021: the path probe is wrapped in Result so that
// permission-denied can be signalled explicitly rather than
// silently returning false. `.isSuccess()` is true iff the probe
// itself worked (even when the path is absent); `.__value` carries
// the Bool answer. We tag the inner as TAIDA_TAG_BOOL so
// polymorphic `.toString()` / stdout prints "true"/"false" rather
// than "1"/"0".
static taida_val taida_os_result_success_bool(taida_val b) {
    taida_val r = taida_os_result_success(b ? 1 : 0);
    taida_pack_set_tag(r, 0, TAIDA_TAG_BOOL);
    return r;
}

taida_val taida_os_exists(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_os_result_failure(EINVAL, "Exists: invalid arguments");
    struct stat st;
    if (stat(path, &st) == 0) {
        return taida_os_result_success_bool(1);
    }
    if (errno == ENOENT || errno == ENOTDIR) {
        // Path simply does not exist — probe itself succeeded.
        return taida_os_result_success_bool(0);
    }
    // Other errors (EACCES, ELOOP, ENAMETOOLONG, EIO ...) are genuine
    // probe failures that deserve a Result failure payload.
    return taida_os_result_failure(errno, strerror(errno));
}

// ── EnvVar[name]() → Lax[Str] ─────────────────────────────
static taida_val taida_os_env_var_lax_error(const char *kind) {
    taida_val error = taida_make_error_with_kind_code("IoError", "EnvVar error", kind, 0);
    return taida_lax_empty_error((taida_val)"", error);
}

taida_val taida_os_env_var(taida_val name_ptr) {
    const char *name = (const char*)name_ptr;
    if (!name) return taida_os_env_var_lax_error("invalid");
    const char *val = getenv(name);
    if (!val) return taida_os_env_var_lax_error("not_found");
    char *copy = taida_str_new_copy(val);
    return taida_lax_new((taida_val)copy, (taida_val)"");
}

// ── F56 Phase 2: MoltenizeSecretFromEnv[name]() → Lax[Secret[Str]] ──
// Reads the env var straight into a sealed carrier. Both the success value and
// the failure-channel default are sealed (never a plain Str on the surface).
taida_val taida_os_env_var_secret(taida_val name_ptr) {
    const char *name = (const char*)name_ptr;
    taida_val empty_secret = taida_secret_new((taida_val)taida_str_new_copy(""));
    if (!name) {
        taida_val error = taida_make_error_with_kind_code(
            "IoError", "MoltenizeSecretFromEnv error", "invalid", 0);
        return taida_lax_empty_error(empty_secret, error);
    }
    const char *val = getenv(name);
    if (!val) {
        taida_val error = taida_make_error_with_kind_code(
            "IoError", "MoltenizeSecretFromEnv error", "not_found", 0);
        return taida_lax_empty_error(empty_secret, error);
    }
    taida_val sealed = taida_secret_new((taida_val)taida_str_new_copy(val));
    return taida_lax_new(sealed, empty_secret);
}

// ── F56 Phase 6+: MoltenizeSecretFromFile[path]() → Async[Lax[Secret[Bytes]]] ──
// Reads the file's bytes straight into a sealed carrier, wrapped in a fulfilled
// Async (the `>=>` await returns immediately), mirroring the interpreter. Both
// the success value and the failure-channel default are sealed Bytes.
taida_val taida_os_secret_from_file(taida_val path_ptr) {
    taida_val empty_secret =
        taida_secret_new(taida_bytes_contig_new((const unsigned char *)"", 0));
    taida_val lax_bytes = taida_os_read_bytes(path_ptr); // Lax[Bytes]
    taida_val lax_secret;
    if (taida_lax_has_value(lax_bytes)) {
        taida_val sealed = taida_secret_new(taida_lax_unmold((taida_ptr)lax_bytes));
        lax_secret = taida_lax_new(sealed, empty_secret);
    } else {
        taida_val error = taida_make_error_with_kind_code(
            "IoError", "MoltenizeSecretFromFile error", "not_found", 0);
        lax_secret = taida_lax_empty_error(empty_secret, error);
    }
    return taida_async_ok_tagged(lax_secret, TAIDA_TAG_PACK);
}

// ── F56 Phase 6+: MoltenizeSecretFromInput[prompt]() → Async[Lax[Secret[Str]]] ──
// Reads a stdin line into a sealed carrier. Reuses taida_io_stdin_line (which
// returns Async[Lax[Str]]), unwraps it, seals the line, and re-wraps.
taida_val taida_os_secret_from_input(taida_val prompt_ptr) {
    taida_val empty_secret = taida_secret_new((taida_val)taida_str_new_copy(""));
    taida_val async_line = taida_io_stdin_line((taida_ptr)prompt_ptr); // Async[Lax[Str]]
    taida_val lax_line = taida_async_unmold((taida_ptr)async_line);    // Lax[Str]
    taida_val lax_secret;
    if (taida_lax_has_value(lax_line)) {
        taida_val sealed = taida_secret_new(taida_lax_unmold((taida_ptr)lax_line));
        lax_secret = taida_lax_new(sealed, empty_secret);
    } else {
        lax_secret = taida_lax_empty(empty_secret);
    }
    return taida_async_ok_tagged(lax_secret, TAIDA_TAG_PACK);
}

// ── writeFile(path, content) → Result[Int] ─────────────────
//
// C12B-021: the Result's inner value is the byte count written,
// not an ok-status BuchiPack. Aligns with Interpreter / JS.
taida_val taida_os_write_file(taida_val path_ptr, taida_val content_ptr) {
    const char *path = (const char*)path_ptr;
    const char *content = (const char*)content_ptr;
    if (!path || !content) return taida_os_result_failure(EINVAL, "writeFile: invalid arguments");

    FILE *f = fopen(path, "w");
    if (!f) return taida_os_result_failure(errno, strerror(errno));

    size_t len = strlen(content);
    size_t written = fwrite(content, 1, len, f);
    fclose(f);

    if (written != len) return taida_os_result_failure(errno, strerror(errno));
    return taida_os_result_success((taida_val)len);
}

// ── writeBytes(path, content) → Result ─────────────────────
taida_val taida_os_write_bytes(taida_val path_ptr, taida_val content_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_os_result_failure(EINVAL, "writeBytes: invalid arguments");

    unsigned char *payload_buf = NULL;
    size_t payload_len = 0;
    if (TAIDA_IS_BYTES(content_ptr)) {
        taida_val *bytes = (taida_val*)content_ptr;
        taida_val len = bytes[1];
        if (len < 0) return taida_os_result_failure(EINVAL, "writeBytes: invalid bytes payload");
        // M-15: Cap bytes len to 256MB to prevent unbounded malloc.
        if (len > (taida_val)(256 * 1024 * 1024)) return taida_os_result_failure(EINVAL, "writeBytes: payload too large");
        payload_buf = (unsigned char*)TAIDA_MALLOC((size_t)len, "writeBytes_payload");
        for (taida_val i = 0; i < len; i++) payload_buf[i] = (unsigned char)bytes[2 + i];
        payload_len = (size_t)len;
    } else {
        const char *content = (const char*)content_ptr;
        size_t content_len = 0;
        if (!taida_read_cstr_len_safe(content, 65536, &content_len)) {
            return taida_os_result_failure(EINVAL, "writeBytes: invalid data");
        }
        payload_buf = (unsigned char*)TAIDA_MALLOC(content_len, "writeBytes_payload");
        memcpy(payload_buf, content, content_len);
        payload_len = content_len;
    }

    FILE *f = fopen(path, "wb");
    if (!f) {
        free(payload_buf);
        return taida_os_result_failure(errno, strerror(errno));
    }

    size_t written = 0;
    if (payload_len > 0) {
        written = fwrite(payload_buf, 1, payload_len, f);
    }
    int saved_errno = errno;
    fclose(f);
    free(payload_buf);

    if (written != payload_len) return taida_os_result_failure(saved_errno, strerror(saved_errno));
    // C12B-021: inner value is the byte count written.
    return taida_os_result_success((taida_val)payload_len);
}

// ── appendFile(path, content) → Result[Int] ────────────────
taida_val taida_os_append_file(taida_val path_ptr, taida_val content_ptr) {
    const char *path = (const char*)path_ptr;
    const char *content = (const char*)content_ptr;
    if (!path || !content) return taida_os_result_failure(EINVAL, "appendFile: invalid arguments");

    FILE *f = fopen(path, "a");
    if (!f) return taida_os_result_failure(errno, strerror(errno));

    size_t len = strlen(content);
    size_t written = fwrite(content, 1, len, f);
    fclose(f);

    if (written != len) return taida_os_result_failure(errno, strerror(errno));
    // C12B-021: inner value is the byte count appended.
    return taida_os_result_success((taida_val)len);
}

// ── remove(path) → Result[Int] ─────────────────────────────
// Recursive removal helper (also counts removed entries as side
// effect via the `count` out-parameter). A file removal counts as 1;
// removing a directory counts the directory itself + every descendant.
static int taida_os_remove_recursive(const char *path, int64_t *count_out) {
    struct stat st;
    if (lstat(path, &st) != 0) return -1;

    if (S_ISDIR(st.st_mode)) {
        DIR *dir = opendir(path);
        if (!dir) return -1;
        struct dirent *entry;
        while ((entry = readdir(dir)) != NULL) {
            if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) continue;
            size_t pathlen = strlen(path) + strlen(entry->d_name) + 2;
            char *child = (char*)TAIDA_MALLOC(pathlen, "remove_recursive");
            snprintf(child, pathlen, "%s/%s", path, entry->d_name);
            int r = taida_os_remove_recursive(child, count_out);
            free(child);
            if (r != 0) { closedir(dir); return -1; }
        }
        closedir(dir);
        if (rmdir(path) != 0) return -1;
        if (count_out) *count_out += 1; // include the dir itself
        return 0;
    } else {
        if (unlink(path) != 0) return -1;
        if (count_out) *count_out += 1;
        return 0;
    }
}

// C12B-021: inner Int value is the number of entries removed
// (1 for a file; directory + descendants for a tree).
taida_val taida_os_remove(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_os_result_failure(EINVAL, "remove: invalid arguments");

    int64_t count = 0;
    if (taida_os_remove_recursive(path, &count) != 0) {
        return taida_os_result_failure(errno, strerror(errno));
    }
    return taida_os_result_success((taida_val)count);
}

// ── createDir(path) → Result[Int] (mkdir -p) ───────────────
static int taida_os_mkdir_p(const char *path) {
    size_t path_len = strlen(path);
    // M-14: Note: mkdir_p returns -1 on failure rather than aborting, so we
    // keep the manual malloc + NULL check pattern here (TAIDA_MALLOC would abort).
    char *tmp = (char*)malloc(path_len + 1);
    if (!tmp) return -1;
    memcpy(tmp, path, path_len + 1);
    for (char *p = tmp + 1; *p; p++) {
        if (*p == '/') {
            *p = '\0';
            if (mkdir(tmp, 0755) != 0 && errno != EEXIST) {
                free(tmp);
                return -1;
            }
            *p = '/';
        }
    }
    int r = mkdir(tmp, 0755);
    free(tmp);
    if (r != 0 && errno != EEXIST) return -1;
    return 0;
}

// C12B-021: inner Int value is 1 if the leaf directory was newly
// created, 0 if it already existed. This matches the Interpreter /
// JS contract so `createDir(p).__value == 1` is the unambiguous
// "I just created this directory" test.
taida_val taida_os_create_dir(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_os_result_failure(EINVAL, "createDir: invalid arguments");

    struct stat pre;
    int already = (stat(path, &pre) == 0 && S_ISDIR(pre.st_mode)) ? 1 : 0;
    if (taida_os_mkdir_p(path) != 0) {
        return taida_os_result_failure(errno, strerror(errno));
    }
    return taida_os_result_success((taida_val)(already ? 0 : 1));
}

// ── rename(from, to) → Result ──────────────────────────────
taida_val taida_os_rename(taida_val from_ptr, taida_val to_ptr) {
    const char *from = (const char*)from_ptr;
    const char *to = (const char*)to_ptr;
    if (!from || !to) return taida_os_result_failure(EINVAL, "rename: invalid arguments");

    if (rename(from, to) != 0) {
        return taida_os_result_failure(errno, strerror(errno));
    }
    return taida_os_result_success(taida_os_ok_inner());
}

// ── run(program, args) → Gorillax[@(stdout, stderr, code)] ──
taida_val taida_os_run(taida_val program_ptr, taida_val args_list_ptr) {
    const char *program = (const char*)program_ptr;
    if (!program) return taida_gorillax_err(taida_make_io_error(EINVAL, "run: invalid arguments"));

    // Build argv from list
    taida_val *list = (taida_val*)args_list_ptr;
    taida_val argc = list ? list[2] : 0;

    // Create pipes for stdout and stderr
    int stdout_pipe[2], stderr_pipe[2];
    if (pipe(stdout_pipe) != 0 || pipe(stderr_pipe) != 0) {
        return taida_gorillax_err(taida_make_io_error(errno, strerror(errno)));
    }

    pid_t pid = fork();
    if (pid < 0) {
        close(stdout_pipe[0]); close(stdout_pipe[1]);
        close(stderr_pipe[0]); close(stderr_pipe[1]);
        return taida_gorillax_err(taida_make_io_error(errno, strerror(errno)));
    }

    if (pid == 0) {
        // Child
        close(stdout_pipe[0]);
        close(stderr_pipe[0]);
        dup2(stdout_pipe[1], STDOUT_FILENO);
        dup2(stderr_pipe[1], STDERR_FILENO);
        close(stdout_pipe[1]);
        close(stderr_pipe[1]);

        // Build argv: [program, arg0, arg1, ..., NULL]
        // M-14: TAIDA_MALLOC ensures NULL check + OOM diagnostic.
        char **argv = (char**)TAIDA_MALLOC((argc + 2) * sizeof(char*), "exec_argv");
        argv[0] = (char*)program;
        for (taida_val i = 0; i < argc; i++) {
            argv[i + 1] = (char*)list[4 + i];
        }
        argv[argc + 1] = NULL;

        execvp(program, argv);
        // If exec fails
        _exit(127);
    }

    // Parent
    close(stdout_pipe[1]);
    close(stderr_pipe[1]);

    // G3: drain stdout + stderr concurrently. Reading one pipe to EOF before
    // the other deadlocks if the child writes >64KB to the undrained pipe.
    char *out_buf, *err_buf;
    size_t out_len, err_len;
    taida_os_drain_two_pipes(stdout_pipe[0], stderr_pipe[0],
                             &out_buf, &out_len, &err_buf, &err_len);
    (void)out_len; (void)err_len; // process_inner uses NUL-terminated buffers

    int status;
    waitpid(pid, &status, 0);
    taida_val exit_code = WIFEXITED(status) ? WEXITSTATUS(status) : -1;

    if (exit_code == 0) {
        taida_val inner = taida_os_process_inner(out_buf, err_buf, exit_code);
        free(out_buf);
        free(err_buf);
        return taida_gorillax_new(inner);
    } else {
        free(out_buf);
        free(err_buf);
        char msg[256];
        snprintf(msg, sizeof(msg), "Process '%s' exited with code %" PRId64 "", program, exit_code);
        taida_val error = taida_make_error("ProcessError", msg);
        return taida_gorillax_err(error);
    }
}

// ── execShell(command) → Gorillax[@(stdout, stderr, code)] ──
taida_val taida_os_exec_shell(taida_val command_ptr) {
    const char *command = (const char*)command_ptr;
    if (!command) return taida_gorillax_err(taida_make_io_error(EINVAL, "execShell: invalid arguments"));

    // Use fork + sh -c to capture both stdout and stderr separately
    int stdout_pipe[2], stderr_pipe[2];
    if (pipe(stdout_pipe) != 0 || pipe(stderr_pipe) != 0) {
        return taida_gorillax_err(taida_make_io_error(errno, strerror(errno)));
    }

    pid_t pid = fork();
    if (pid < 0) {
        close(stdout_pipe[0]); close(stdout_pipe[1]);
        close(stderr_pipe[0]); close(stderr_pipe[1]);
        return taida_gorillax_err(taida_make_io_error(errno, strerror(errno)));
    }

    if (pid == 0) {
        close(stdout_pipe[0]);
        close(stderr_pipe[0]);
        dup2(stdout_pipe[1], STDOUT_FILENO);
        dup2(stderr_pipe[1], STDERR_FILENO);
        close(stdout_pipe[1]);
        close(stderr_pipe[1]);
        execl("/bin/sh", "sh", "-c", command, (char*)NULL);
        _exit(127);
    }

    close(stdout_pipe[1]);
    close(stderr_pipe[1]);

    // G3: drain stdout + stderr concurrently (see taida_os_drain_two_pipes).
    char *out_buf, *err_buf;
    size_t out_len, err_len;
    taida_os_drain_two_pipes(stdout_pipe[0], stderr_pipe[0],
                             &out_buf, &out_len, &err_buf, &err_len);
    (void)out_len; (void)err_len;

    int status;
    waitpid(pid, &status, 0);
    taida_val exit_code = WIFEXITED(status) ? WEXITSTATUS(status) : -1;

    if (exit_code == 0) {
        taida_val inner = taida_os_process_inner(out_buf, err_buf, exit_code);
        free(out_buf);
        free(err_buf);
        return taida_gorillax_new(inner);
    } else {
        free(out_buf);
        free(err_buf);
        char msg[256];
        snprintf(msg, sizeof(msg), "Shell command exited with code %" PRId64 ": %s", exit_code, command);
        taida_val error = taida_make_error("ProcessError", msg);
        return taida_gorillax_err(error);
    }
}

// ── C19: runInteractive(program, args) → Gorillax[@(code: Int)] ──
//
// TTY-passthrough variant of `run`. The child inherits the parent's
// stdin/stdout/stderr so it can draw TUIs (nvim, less, fzf, git commit).
//
// Contract (must match interpreter and JS exactly):
// - Success (exit 0): Gorillax.ok(@(code = 0))
// - Non-zero exit: Gorillax.err(ProcessError{code})
// - Pre-exec / fork failure: Gorillax.err(IoError{errno, kind})
// - ENOENT / exec failure: Gorillax.err(IoError{errno, kind})
// - Signal death: code = 128 + signum (best-effort)
//
// Critical difference from `taida_os_run`:
// - NO pipe() calls for stdio (child keeps the parent's TTY FDs)
// - NO dup2() in the child
// - NO read() in the parent (nothing to drain)
//
// The errno pipe (CLOEXEC) is separate from stdio: it is used only to
// transport the errno of a failed `execvp` from child to parent so that
// ENOENT / EACCES etc. surface as `IoError` rather than `ProcessError(127)`.
// On successful exec the pipe auto-closes (CLOEXEC) and the parent's
// read() returns 0 bytes, indicating "exec succeeded; child is running".

// Write-all helper (handles short writes / EINTR). Returns 0 on success,
// -1 on failure. Used only by the child to push errno to the parent.
static int taida_os_write_all(int fd, const void *buf, size_t len) {
    const unsigned char *p = (const unsigned char*)buf;
    size_t remaining = len;
    while (remaining > 0) {
        ssize_t n = write(fd, p, remaining);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        p += (size_t)n;
        remaining -= (size_t)n;
    }
    return 0;
}

// Read-all helper (handles short reads / EINTR). Returns the number of
// bytes actually read, or -1 on error. Used by the parent to drain the
// errno pipe after the child exits.
static ssize_t taida_os_read_all(int fd, void *buf, size_t len) {
    unsigned char *p = (unsigned char*)buf;
    size_t total = 0;
    while (total < len) {
        ssize_t n = read(fd, p + total, len - total);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (n == 0) break; // EOF (pipe closed on successful exec)
        total += (size_t)n;
    }
    return (ssize_t)total;
}

taida_val taida_os_run_interactive(taida_val program_ptr, taida_val args_list_ptr) {
    const char *program = (const char*)program_ptr;
    if (!program) return taida_gorillax_err(taida_make_io_error(EINVAL, "runInteractive: invalid arguments"));

    taida_val *list = (taida_val*)args_list_ptr;
    taida_val argc = list ? list[2] : 0;

    // CLOEXEC error pipe: child writes errno here if execvp fails; on
    // successful exec the kernel auto-closes both ends so the parent
    // sees EOF.
    int err_pipe[2];
    if (pipe(err_pipe) != 0) {
        return taida_gorillax_err(taida_make_io_error(errno, strerror(errno)));
    }
    if (fcntl(err_pipe[0], F_SETFD, FD_CLOEXEC) != 0 ||
        fcntl(err_pipe[1], F_SETFD, FD_CLOEXEC) != 0) {
        int saved = errno;
        close(err_pipe[0]);
        close(err_pipe[1]);
        return taida_gorillax_err(taida_make_io_error(saved, strerror(saved)));
    }

    pid_t pid = fork();
    if (pid < 0) {
        int saved = errno;
        close(err_pipe[0]);
        close(err_pipe[1]);
        return taida_gorillax_err(taida_make_io_error(saved, strerror(saved)));
    }

    if (pid == 0) {
        // Child: do NOT dup2 stdio. The error pipe's read end is unused here;
        // the write end stays open and is CLOEXEC so it vanishes on
        // successful execvp.
        close(err_pipe[0]);

        char **argv = (char**)TAIDA_MALLOC((argc + 2) * sizeof(char*), "exec_argv_interactive");
        argv[0] = (char*)program;
        for (taida_val i = 0; i < argc; i++) {
            argv[i + 1] = (char*)list[4 + i];
        }
        argv[argc + 1] = NULL;

        execvp(program, argv);
        // execvp failed — push errno to the parent before dying.
        int exec_errno = errno;
        (void)taida_os_write_all(err_pipe[1], &exec_errno, sizeof(exec_errno));
        close(err_pipe[1]);
        _exit(127);
    }

    // Parent: close the write end (child holds its copy), then drain the
    // error pipe. If the pipe yields sizeof(int) bytes, execvp failed and
    // that int is the child's errno. If it yields 0 bytes (EOF), execvp
    // succeeded and the child is running / has exited normally.
    close(err_pipe[1]);

    int child_errno = 0;
    ssize_t got = taida_os_read_all(err_pipe[0], &child_errno, sizeof(child_errno));
    close(err_pipe[0]);

    int status;
    waitpid(pid, &status, 0);

    if (got == (ssize_t)sizeof(child_errno) && child_errno != 0) {
        // execvp failed — classify as IoError, not ProcessError.
        return taida_gorillax_err(taida_make_io_error(child_errno, strerror(child_errno)));
    }

    taida_val exit_code = taida_os_extract_wait_code(status);
    taida_val inner = taida_os_process_inner_code_only(exit_code);
    if (exit_code == 0) {
        return taida_gorillax_new(inner);
    }
    char msg[256];
    snprintf(msg, sizeof(msg), "Process '%s' exited with code %" PRId64, program, exit_code);
    taida_val error = taida_make_error("ProcessError", msg);
    taida_release(inner);
    return taida_gorillax_err(error);
}

// ── C19: execShellInteractive(command) → Gorillax[@(code: Int)] ──
//
// TTY-passthrough variant of `execShell`. Uses `sh -c` (no cmd /C branch
// here — Windows would use _spawnvp and is best-effort).
//
// Uses the same CLOEXEC errno-pipe pattern as `taida_os_run_interactive`
// so that failures to even spawn `/bin/sh` surface as IoError. In
// practice this path is almost never ENOENT (sh is always present) but
// we keep the classification consistent for symmetry.
taida_val taida_os_exec_shell_interactive(taida_val command_ptr) {
    const char *command = (const char*)command_ptr;
    if (!command) return taida_gorillax_err(taida_make_io_error(EINVAL, "execShellInteractive: invalid arguments"));

    int err_pipe[2];
    if (pipe(err_pipe) != 0) {
        return taida_gorillax_err(taida_make_io_error(errno, strerror(errno)));
    }
    if (fcntl(err_pipe[0], F_SETFD, FD_CLOEXEC) != 0 ||
        fcntl(err_pipe[1], F_SETFD, FD_CLOEXEC) != 0) {
        int saved = errno;
        close(err_pipe[0]);
        close(err_pipe[1]);
        return taida_gorillax_err(taida_make_io_error(saved, strerror(saved)));
    }

    pid_t pid = fork();
    if (pid < 0) {
        int saved = errno;
        close(err_pipe[0]);
        close(err_pipe[1]);
        return taida_gorillax_err(taida_make_io_error(saved, strerror(saved)));
    }

    if (pid == 0) {
        // Child: no dup2 — inherit parent's TTY FDs.
        close(err_pipe[0]);
        execl("/bin/sh", "sh", "-c", command, (char*)NULL);
        int exec_errno = errno;
        (void)taida_os_write_all(err_pipe[1], &exec_errno, sizeof(exec_errno));
        close(err_pipe[1]);
        _exit(127);
    }

    close(err_pipe[1]);

    int child_errno = 0;
    ssize_t got = taida_os_read_all(err_pipe[0], &child_errno, sizeof(child_errno));
    close(err_pipe[0]);

    int status;
    waitpid(pid, &status, 0);

    if (got == (ssize_t)sizeof(child_errno) && child_errno != 0) {
        return taida_gorillax_err(taida_make_io_error(child_errno, strerror(child_errno)));
    }

    taida_val exit_code = taida_os_extract_wait_code(status);
    taida_val inner = taida_os_process_inner_code_only(exit_code);
    if (exit_code == 0) {
        return taida_gorillax_new(inner);
    }
    char msg[256];
    snprintf(msg, sizeof(msg), "Shell command exited with code %" PRId64 ": %s", exit_code, command);
    taida_val error = taida_make_error("ProcessError", msg);
    taida_release(inner);
    return taida_gorillax_err(error);
}

// ── allEnv() → HashMap[Str, Str] ──────────────────────────
extern char **environ;

taida_val taida_os_all_env(void) {
    // F-24 fix: count env vars and set initial capacity accordingly
    taida_val env_count = 0;
    if (environ) {
        for (char **e = environ; *e; e++) env_count++;
    }
    // Capacity should be at least 2x entries for good load factor
    taida_val init_cap = 16;
    while (init_cap * 3 < env_count * 4) init_cap *= 2;  // ensure load < 0.75
    taida_val hm = taida_hashmap_new_with_cap(init_cap);
    // NO-1: allEnv returns HashMap[Str, Str] — set value_type_tag
    taida_hashmap_set_value_tag(hm, TAIDA_TAG_STR);
    if (!environ) return hm;
    for (char **env = environ; *env; env++) {
        char *eq = strchr(*env, '=');
        if (!eq) continue;
        size_t key_len = eq - *env;
        char *key = taida_str_alloc(key_len);
        memcpy(key, *env, key_len);
        char *val = taida_str_new_copy(eq + 1);
        taida_val key_hash = taida_str_hash((taida_val)key);
        hm = taida_hashmap_set(hm, key_hash, (taida_val)key, (taida_val)val);
    }
    return hm;
}

taida_val taida_os_argv(void) {
    taida_val list = taida_list_new();
    if (!taida_cli_argv || taida_cli_argc <= 1) return list;
    // Native binary mode: <program> [args...]
    for (int i = 1; i < taida_cli_argc; i++) {
        const char *arg = taida_cli_argv[i] ? taida_cli_argv[i] : "";
        list = taida_list_push(list, (taida_val)taida_str_new_copy(arg));
    }
    return list;
}

// ── Phase 2: Async OS APIs (pthread-based) ────────────────
// These APIs use pthread to run blocking operations in a background thread,
// returning an Async value that resolves when the thread completes.

#include <sys/socket.h>
#include <netdb.h>
#include <arpa/inet.h>
#include <netinet/in.h>
#include <sys/time.h>
#include <sys/uio.h>  // NET3-5c: writev() for zero-copy chunk writes
#include <signal.h>   // NB3-5: SIGPIPE suppression for peer-close resilience
#include <dlfcn.h>    // NET5-4a: dlopen for OpenSSL TLS support
#include <stdbool.h>  // NET7-8a: bool type for quiche FFI
