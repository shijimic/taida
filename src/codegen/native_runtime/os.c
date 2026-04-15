// ── taida-lang/os package — Native runtime ────────────────

// Helper: build os Result success BuchiPack @(ok=true, code=0, message="")
static taida_val taida_os_result_success(taida_val inner) {
    return taida_result_create(inner, 0, 0);
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
    return taida_result_create(inner, error, 0);
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

// ── Read[path]() → Lax[Str] ──────────────────────────────
taida_val taida_os_read(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_lax_empty((taida_val)"");

    // Check file size (64MB limit)
    struct stat st;
    if (stat(path, &st) != 0) return taida_lax_empty((taida_val)"");
    if (st.st_size > 64 * 1024 * 1024) return taida_lax_empty((taida_val)"");

    FILE *f = fopen(path, "r");
    if (!f) return taida_lax_empty((taida_val)"");

    taida_val size = st.st_size;
    char *buf = taida_str_alloc(size);
    taida_val read_bytes = (taida_val)fread(buf, 1, size, f);
    fclose(f);
    buf[read_bytes] = '\0';

    return taida_lax_new((taida_val)buf, (taida_val)"");
}

// ── readBytes(path) → Lax[Bytes] ──────────────────────────
taida_val taida_os_read_bytes(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_lax_empty(taida_bytes_default_value());

    struct stat st;
    if (stat(path, &st) != 0) return taida_lax_empty(taida_bytes_default_value());
    if (st.st_size > 64 * 1024 * 1024) return taida_lax_empty(taida_bytes_default_value());

    FILE *f = fopen(path, "rb");
    if (!f) return taida_lax_empty(taida_bytes_default_value());

    taida_val size = st.st_size;
    unsigned char *buf = NULL;
    if (size > 0) {
        buf = (unsigned char*)malloc((size_t)size);
        if (!buf) {
            fclose(f);
            return taida_lax_empty(taida_bytes_default_value());
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

// ── String comparator for qsort ──────────────────────────
static int taida_cmp_strings(const void *a, const void *b) {
    return strcmp(*(const char**)a, *(const char**)b);
}

// ── ListDir[path]() → Lax[@[Str]] ────────────────────────
taida_val taida_os_list_dir(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;
    if (!path) return taida_lax_empty(taida_list_new());

    DIR *dir = opendir(path);
    if (!dir) return taida_lax_empty(taida_list_new());

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
                return taida_lax_empty(taida_list_new());
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

// ── Stat[path]() → Lax[@(size: Int, modified: Str, isDir: Bool)] ──
taida_val taida_os_stat(taida_val path_ptr) {
    const char *path = (const char*)path_ptr;

    // Build default stat pack
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

    if (!path) return taida_lax_empty(default_pack);

    struct stat st;
    if (stat(path, &st) != 0) return taida_lax_empty(default_pack);

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
    taida_val r = taida_result_create(b ? 1 : 0, 0, 0);
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
taida_val taida_os_env_var(taida_val name_ptr) {
    const char *name = (const char*)name_ptr;
    if (!name) return taida_lax_empty((taida_val)"");
    const char *val = getenv(name);
    if (!val) return taida_lax_empty((taida_val)"");
    char *copy = taida_str_new_copy(val);
    return taida_lax_new((taida_val)copy, (taida_val)"");
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

    // Read stdout
    size_t out_cap = 4096, out_len = 0;
    char *out_buf = (char*)TAIDA_MALLOC(out_cap, "os_run_stdout");
    ssize_t n;
    while ((n = read(stdout_pipe[0], out_buf + out_len, out_cap - out_len - 1)) > 0) {
        out_len += n;
        if (out_len >= out_cap - 1) {
            out_cap *= 2;
            TAIDA_REALLOC(out_buf, out_cap, "os_run_stdout");
        }
    }
    out_buf[out_len] = '\0';
    close(stdout_pipe[0]);

    // Read stderr
    size_t err_cap = 4096, err_len = 0;
    char *err_buf = (char*)TAIDA_MALLOC(err_cap, "os_run_stderr");
    while ((n = read(stderr_pipe[0], err_buf + err_len, err_cap - err_len - 1)) > 0) {
        err_len += n;
        if (err_len >= err_cap - 1) {
            err_cap *= 2;
            TAIDA_REALLOC(err_buf, err_cap, "os_run_stderr");
        }
    }
    err_buf[err_len] = '\0';
    close(stderr_pipe[0]);

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

    size_t out_cap = 4096, out_len = 0;
    char *out_buf = (char*)TAIDA_MALLOC(out_cap, "execShell_stdout");
    ssize_t n;
    while ((n = read(stdout_pipe[0], out_buf + out_len, out_cap - out_len - 1)) > 0) {
        out_len += n;
        if (out_len >= out_cap - 1) { out_cap *= 2; TAIDA_REALLOC(out_buf, out_cap, "execShell_stdout"); }
    }
    out_buf[out_len] = '\0';
    close(stdout_pipe[0]);

    size_t err_cap = 4096, err_len = 0;
    char *err_buf = (char*)TAIDA_MALLOC(err_cap, "execShell_stderr");
    while ((n = read(stderr_pipe[0], err_buf + err_len, err_cap - err_len - 1)) > 0) {
        err_len += n;
        if (err_len >= err_cap - 1) { err_cap *= 2; TAIDA_REALLOC(err_buf, err_cap, "execShell_stderr"); }
    }
    err_buf[err_len] = '\0';
    close(stderr_pipe[0]);

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

