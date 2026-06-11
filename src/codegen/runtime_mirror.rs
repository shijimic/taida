//! Mirror-sync drift gate between the native C runtime and the WASM C
//! runtime.
//!
//! The two runtimes implement a large common surface (~400 `taida_*`
//! symbols) as deliberate per-target sources: native
//! (`native_runtime/*.c`, `taida_val` spelling) and WASM
//! (`runtime_core_wasm/*.inc.c`, `int64_t` spelling). For a core layer
//! (arithmetic, comparisons, thin wrappers) the bodies are required to
//! stay identical modulo that one type-name spelling; the rest diverges
//! deliberately (allocator policy, refcounting, libc/libm availability,
//! pthread vs deterministic async, …).
//!
//! Historically nothing enforced the boundary: editing one side of an
//! identical pair and forgetting the other produced silent drift that
//! only surfaced as a backend parity bug much later. This module pins
//! the boundary mechanically:
//!
//! - [`MIRROR_SYNC_ALLOWLIST`]: common symbols whose definitions must
//!   be identical after normalization (`taida_val` → `int64_t`,
//!   whitespace collapsed). Editing one side without the other fails
//!   the gate test.
//! - [`HELPER_ALLOWLIST`]: non-`taida_*` static helpers referenced by
//!   allowlisted bodies (`_d2l`, `_to_double`). Their definitions must
//!   match too — textual equality of a caller is meaningless if the
//!   helper it calls means something else on the other side.
//! - [`DIVERGENT_GROUPS`]: common symbols whose divergence is
//!   deliberate, grouped by reason. An entry here documents "the two
//!   bodies are *expected* to differ — edit them independently".
//!
//! Every common symbol must appear in exactly one of the two sets; a
//! symbol in neither (e.g. a freshly added function present in both
//! runtimes) fails the classification test until a human decides which
//! contract it lives under. Stale entries (symbols that stopped being
//! common) fail too, so the tables cannot rot in either direction.
//!
//! Demoting a symbol from the allowlist to a divergence group is a
//! design decision, not a quick fix: record the reason in the matching
//! group (or a new one) — the default response to a mirror-drift
//! failure is to finish the half-done edit on the other runtime.
//!
//! Scope: the gate guarantees *textual body identity* of the
//! allowlisted layer (plus the non-`taida_*` helpers it calls). When an
//! allowlisted body calls a deliberately-divergent `taida_*` symbol
//! (e.g. `taida_lax_has_value` → `taida_pack_get_idx`), the callee's
//! cross-target behavioral agreement is NOT this gate's job — that is
//! what the backend parity test suite verifies end to end.
//!
//! The gate is intentionally test-only (no production code reads these
//! tables): it changes zero behavior and exists to make the implicit
//! cross-runtime contract executable.

use std::collections::BTreeMap;

/// Per-byte context for the C scanner: `Some(depth)` when the byte is
/// outside string/char literals and comments (depth = brace nesting
/// before the byte), `None` when it is inside one of those.
fn significant_depths(src: &str) -> Vec<Option<i32>> {
    let b = src.as_bytes();
    let n = b.len();
    let mut out = vec![None; n];
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < n {
        let c = b[i];
        // line comment
        if c == b'/' && i + 1 < n && b[i + 1] == b'/' {
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // block comment
        if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(n);
            continue;
        }
        // string / char literal
        if c == b'"' || c == b'\'' {
            let quote = c;
            i += 1;
            while i < n {
                if b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == quote {
                    break;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        out[i] = Some(depth);
        if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
        }
        i += 1;
    }
    out
}

/// Extract every top-level C function *definition* (signature + body)
/// from `src`, keyed by function name. Token-aware: signatures inside
/// comments/strings and nested scopes are ignored, and prototype
/// declarations (`);`-terminated) are skipped.
///
/// Known limitation: the scanner has no preprocessor model — `#if`
/// branches are scanned as plain text. Today no shared function
/// definition sits inside a conditional block (the `#if` uses in both
/// runtimes wrap macro constants and platform errno shims only); if
/// one ever does, the duplicate-definition check below fails the gate
/// rather than silently picking one branch.
pub(crate) fn enumerate_fn_defs(src: &str) -> BTreeMap<String, String> {
    use regex::Regex;
    use std::sync::OnceLock;
    static SIG: OnceLock<Regex> = OnceLock::new();
    // Each return-type token may carry attached pointer stars
    // (`char* f(...)` / `char *f(...)` / `char * f(...)` all parse).
    let sig = SIG.get_or_init(|| {
        Regex::new(
            r"(?m)^(?:static\s+)?(?:[A-Za-z_][A-Za-z0-9_]*\**\s+)+\**\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(",
        )
        .expect("static signature regex")
    });

    let depths = significant_depths(src);
    let b = src.as_bytes();
    let n = b.len();
    let mut defs = BTreeMap::new();

    for m in sig.captures_iter(src) {
        let whole = m.get(0).expect("whole match");
        let start = whole.start();
        if depths[start] != Some(0) {
            continue; // inside a comment/string or a nested scope
        }
        let name = m.get(1).expect("name group").as_str();
        // Walk from '(' to its matching ')', then decide def vs decl.
        let open_paren = whole.end() - 1;
        let mut pd = 0i32;
        let mut j = open_paren;
        let mut body_start = None;
        while j < n {
            if depths[j].is_some() {
                match b[j] {
                    b'(' => pd += 1,
                    b')' => {
                        pd -= 1;
                        if pd == 0 {
                            let mut k = j + 1;
                            while k < n
                                && (depths[k].is_none() || (b[k] as char).is_ascii_whitespace())
                            {
                                k += 1;
                            }
                            if k < n && b[k] == b'{' {
                                body_start = Some(k);
                            }
                            break;
                        }
                    }
                    _ => {}
                }
            }
            j += 1;
        }
        let Some(body_start) = body_start else {
            continue; // prototype declaration
        };
        let mut bd = 0i32;
        let mut k = body_start;
        while k < n {
            if depths[k].is_some() {
                match b[k] {
                    b'{' => bd += 1,
                    b'}' => {
                        bd -= 1;
                        if bd == 0 {
                            let prev = defs.insert(name.to_string(), src[start..=k].to_string());
                            assert!(
                                prev.is_none(),
                                "duplicate top-level definition of `{name}` — likely a \
                                 preprocessor-conditional pair the scanner cannot \
                                 disambiguate; teach it the `#if` structure first"
                            );
                            break;
                        }
                    }
                    _ => {}
                }
            }
            k += 1;
        }
    }
    defs
}

/// Strip C comments, preserving string/char literal contents verbatim
/// (comment-looking sequences inside literals stay significant).
fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let n = b.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut i = 0usize;
    while i < n {
        let c = b[i];
        if c == b'/' && i + 1 < n && b[i + 1] == b'/' {
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(n);
            out.push(b' ');
            continue;
        }
        if c == b'"' || c == b'\'' {
            let quote = c;
            out.push(c);
            i += 1;
            while i < n {
                if b[i] == b'\\' {
                    out.push(b[i]);
                    if i + 1 < n {
                        out.push(b[i + 1]);
                    }
                    i += 2;
                    continue;
                }
                out.push(b[i]);
                if b[i] == quote {
                    break;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Replace `taida_val` identifier tokens with `int64_t`, leaving
/// string/char literal contents untouched (a literal mentioning the
/// typedef name is data, and rewriting it would let a real message
/// drift slip through normalization). Expects comments to be stripped
/// already.
fn replace_value_typedef(src: &str) -> String {
    let b = src.as_bytes();
    let n = b.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut i = 0usize;
    while i < n {
        let c = b[i];
        if c == b'"' || c == b'\'' {
            let quote = c;
            let start = i;
            i += 1;
            while i < n {
                if b[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if b[i] == quote {
                    break;
                }
                i += 1;
            }
            i = (i + 1).min(n);
            out.extend_from_slice(&b[start..i]);
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            if &b[start..i] == b"taida_val" {
                out.extend_from_slice(b"int64_t");
            } else {
                out.extend_from_slice(&b[start..i]);
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Normalize a definition for cross-runtime comparison. Tolerated
/// differences, and nothing else: the value-typedef spelling
/// (`taida_val` on native, `int64_t` on WASM — identifier tokens only,
/// never inside string literals), comments (the runtimes have
/// different documentation styles — executable drift is what the gate
/// hunts), and whitespace layout.
pub(crate) fn normalize_def(text: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static WS: OnceLock<Regex> = OnceLock::new();
    let ws = WS.get_or_init(|| Regex::new(r"\s+").expect("whitespace regex"));
    let stripped = strip_comments(text);
    let replaced = replace_value_typedef(&stripped);
    ws.replace_all(&replaced, " ").trim().to_string()
}

/// Common symbols whose definitions must stay identical across the two
/// runtimes (modulo the value-typedef spelling). Initial population =
/// every common symbol whose bodies already matched at gate
/// introduction; grow it deliberately, shrink it only as a recorded
/// design decision (see module doc).
pub(crate) const MIRROR_SYNC_ALLOWLIST: &[&str] = &[
    "taida_bool_and",
    "taida_bool_not",
    "taida_bool_or",
    "taida_div_exact",
    "taida_div_mold",
    "taida_error_type_check_or_rethrow",
    "taida_float_add",
    "taida_float_eq",
    "taida_float_gt",
    "taida_float_gte",
    "taida_float_lt",
    "taida_float_lte",
    "taida_float_mul",
    "taida_float_neq",
    "taida_float_sub",
    "taida_int_add",
    "taida_int_clamp",
    "taida_int_eq",
    "taida_int_gt",
    "taida_int_gte",
    "taida_int_is_negative",
    "taida_int_is_positive",
    "taida_int_is_zero",
    "taida_int_lt",
    "taida_int_mul",
    "taida_int_neg",
    "taida_int_neq",
    "taida_int_sub",
    "taida_json_stringify",
    "taida_json_to_str",
    "taida_lax_get_or_default",
    "taida_lax_has_value",
    "taida_lax_is_empty",
    "taida_lax_unmold",
    "taida_mod_exact",
    "taida_mod_mold",
    "taida_poly_neq_tagged",
];

/// Non-`taida_*` static helpers that allowlisted bodies call (directly
/// or transitively: `_to_double` itself calls `_l2d`). They are outside
/// the common-symbol enumeration (no `taida_` prefix) but their
/// definitions must match for the allowlist equalities to be
/// meaningful.
pub(crate) const HELPER_ALLOWLIST: &[&str] = &["_d2l", "_l2d", "_to_double"];

/// Deliberately divergent: async is pthread worker/aggregation based on
/// native; WASM ships deterministic single-threaded stubs.
const DIVERGENT_ASYNC: &[&str] = &[
    "taida_async_all",
    "taida_async_cancel",
    "taida_async_err",
    "taida_async_get_error",
    "taida_async_get_or_default",
    "taida_async_get_value",
    "taida_async_is_fulfilled",
    "taida_async_is_pending",
    "taida_async_is_rejected",
    "taida_async_map",
    "taida_async_ok",
    "taida_async_ok_tagged",
    "taida_async_race",
    "taida_async_set_value_tag",
    "taida_async_spawn",
    "taida_async_task_new",
    "taida_async_task_par",
    "taida_async_task_par_map",
    "taida_async_unmold",
];

/// Deliberately divergent: native writes through libc stdio; WASM goes
/// through `fd_write`-style imports with its own buffering.
const DIVERGENT_IO_DEBUG: &[&str] = &[
    "taida_debug_bool",
    "taida_debug_float",
    "taida_debug_int",
    "taida_debug_json",
    "taida_debug_list",
    "taida_debug_polymorphic",
    "taida_debug_str",
    "taida_io_stderr",
    "taida_io_stderr_with_tag",
    "taida_io_stdout",
    "taida_io_stdout_with_tag",
];

/// Deliberately divergent: crypto primitives share the algorithms but
/// differ in buffer/alloc plumbing between the targets.
const DIVERGENT_CRYPTO: &[&str] = &[
    "taida_crypto_base64_encode",
    "taida_crypto_constant_time_equals",
    "taida_crypto_hex_encode",
    "taida_crypto_hmac_sha256",
    "taida_crypto_sha224",
    "taida_crypto_sha384",
    "taida_crypto_sha512",
    // F56 Phase 4: secret-aware consumers reveal via the per-target carrier
    // probe (`taida_is_moltenized` / `_wasm_carrier_kind`) then share the
    // crypto algorithm — same divergence class as the primitives above.
    "taida_constant_time_eq_secret",
    "taida_hmac_sha256_secret",
];

/// Deliberately divergent: the iteration-scope watermark rides each
/// backend's own allocator — native encodes {active_chunk, offset} into
/// the mark, frees chunks past it and gates the freelists via the
/// thread-local depth; WASM wraps the single bump-pointer
/// wasm_arena_enter/leave pair (and its call sites stay disabled in the
/// emitter until WASM strings carry hidden headers).
const DIVERGENT_ITER_SCOPE: &[&str] = &[
    "taida_arena_iter_enter",
    "taida_arena_iter_reset",
    "taida_arena_iter_exit",
];

/// Deliberately divergent: container construction/mutation sits on
/// different memory policies — native = freelist + arena + live
/// refcounting; WASM = bump allocator + no-op refcounting. Element-kind
/// tag bookkeeping rides the same code paths.
const DIVERGENT_CONTAINER_ALLOC_RC: &[&str] = &[
    "taida_closure_get_env",
    "taida_closure_get_fn",
    "taida_closure_new",
    "taida_hashmap_adjust_hash",
    "taida_hashmap_clone",
    "taida_hashmap_entries",
    "taida_hashmap_get",
    "taida_hashmap_get_lax",
    "taida_hashmap_has",
    "taida_hashmap_is_empty",
    "taida_hashmap_key_eq",
    "taida_hashmap_key_release",
    "taida_hashmap_key_retain",
    "taida_hashmap_key_valid",
    "taida_hashmap_keys",
    "taida_hashmap_length",
    "taida_hashmap_merge",
    "taida_hashmap_new",
    "taida_hashmap_new_with_cap",
    "taida_hashmap_remove",
    "taida_hashmap_remove_immut",
    "taida_hashmap_resize",
    "taida_hashmap_set",
    "taida_hashmap_set_immut",
    "taida_hashmap_set_internal",
    "taida_hashmap_set_value_tag",
    "taida_hashmap_to_string",
    "taida_hashmap_val_release",
    "taida_hashmap_val_retain",
    "taida_hashmap_values",
    "taida_list_all",
    "taida_list_any",
    "taida_list_append",
    "taida_list_append_k",
    "taida_list_append_consume",
    "taida_list_append_consume_k",
    "taida_list_concat",
    "taida_list_contains",
    "taida_list_count",
    "taida_list_drop",
    "taida_list_drop_while",
    "taida_list_elem_release",
    "taida_list_elem_retain",
    "taida_list_enumerate",
    "taida_list_filter",
    "taida_list_find",
    "taida_list_find_index",
    "taida_list_find_index_lax",
    "taida_list_first",
    "taida_list_flatten",
    "taida_list_fold",
    "taida_list_foldr",
    "taida_list_get",
    "taida_list_index_of",
    "taida_list_is_empty",
    "taida_list_join",
    "taida_list_last",
    "taida_list_last_index_of",
    "taida_list_length",
    "taida_list_map",
    "taida_list_map_k",
    "taida_list_max",
    "taida_list_min",
    "taida_list_new",
    "taida_list_none",
    "taida_list_note_push_ekind",
    "taida_list_prepend",
    "taida_list_push",
    "taida_list_reverse",
    "taida_list_set_elem_tag",
    "taida_list_sort",
    "taida_list_sort_by",
    "taida_list_sort_desc",
    "taida_list_sum",
    "taida_list_take",
    "taida_list_take_while",
    "taida_list_to_display_string",
    "taida_list_unique",
    "taida_list_unique_by",
    "taida_list_zip",
    "taida_pack_call_field0",
    "taida_pack_call_field1",
    "taida_pack_call_field2",
    "taida_pack_call_field3",
    "taida_pack_get",
    "taida_pack_get_field_tag",
    "taida_pack_get_idx",
    "taida_pack_has_hash",
    "taida_pack_new",
    "taida_pack_set",
    "taida_pack_set_hash",
    "taida_pack_set_tag",
    "taida_pack_to_display_string",
    "taida_release",
    "taida_retain",
    "taida_retain_and_tag_field",
    "taida_set_add",
    "taida_set_add_tagged",
    "taida_set_call_arg_tag",
    "taida_set_contains",
    "taida_set_diff",
    "taida_set_from_list",
    "taida_set_has",
    "taida_set_has_tagged",
    "taida_set_intersect",
    "taida_set_is_empty",
    "taida_set_new",
    "taida_set_remove",
    "taida_set_remove_k",
    "taida_set_return_tag",
    "taida_set_set_elem_tag",
    "taida_set_size",
    "taida_set_to_list",
    "taida_set_to_string",
    "taida_set_union",
    "taida_str_alloc",
    "taida_str_concat",
    "taida_str_from_bool",
    "taida_str_from_float",
    "taida_str_from_int",
    "taida_str_new_copy",
    "taida_str_repeat",
    "taida_str_replace",
    "taida_str_replace_first",
    "taida_str_replace_first_poly",
    "taida_str_replace_poly",
    "taida_str_slice",
    "taida_str_split",
    "taida_str_split_poly",
    "taida_str_trim",
    "taida_str_trim_end",
    "taida_str_trim_start",
];

/// Deliberately divergent: polymorphic dispatch / type predicates need
/// pointer-validity and heap-shape detection that differs per target
/// (native probes address ranges; WASM uses bump-heap bounds).
const DIVERGENT_POLY_STR_DETECT: &[&str] = &[
    "taida_cmp_strings",
    "taida_is_async",
    "taida_is_buchi_pack",
    "taida_is_bytes",
    "taida_is_closure_value",
    "taida_is_hashmap",
    "taida_is_list",
    "taida_is_molten",
    "taida_is_set",
    "taida_is_string_value",
    "taida_poly_add",
    "taida_poly_eq",
    "taida_poly_eq_tagged",
    "taida_poly_neq",
    "taida_polymorphic_contains",
    "taida_polymorphic_get_or_default",
    "taida_polymorphic_has_value",
    "taida_polymorphic_index_of",
    "taida_polymorphic_index_of_lax",
    "taida_polymorphic_is_empty",
    "taida_polymorphic_last_index_of",
    "taida_polymorphic_last_index_of_lax",
    "taida_polymorphic_length",
    "taida_polymorphic_map",
    "taida_polymorphic_to_string",
    "taida_typeof",
];

/// Deliberately divergent: conversions and float math — native links
/// libm and libc printf-family; WASM carries portable reimplementations
/// (taylor/iterative math, manual ftoa/itoa).
const DIVERGENT_MOLD_CONV: &[&str] = &[
    "taida_bool_mold_bool",
    "taida_bool_mold_float",
    "taida_bool_mold_int",
    "taida_bool_mold_str",
    "taida_bool_to_int",
    "taida_bool_to_str",
    "taida_char_mold_int",
    "taida_char_mold_str",
    "taida_char_to_digit",
    "taida_codepoint_mold_str",
    "taida_digit_to_char",
    "taida_float_abs",
    "taida_float_acos",
    "taida_float_asin",
    "taida_float_atan",
    "taida_float_atan2",
    "taida_float_ceil",
    "taida_float_clamp",
    "taida_float_cos",
    "taida_float_cosh",
    "taida_float_exp",
    "taida_float_floor",
    "taida_float_is_finite_check",
    "taida_float_is_infinite",
    "taida_float_is_nan",
    "taida_float_is_negative",
    "taida_float_is_positive",
    "taida_float_is_zero",
    "taida_float_ln",
    "taida_float_log",
    "taida_float_log10",
    "taida_float_log2",
    "taida_float_mold_bool",
    "taida_float_mold_float",
    "taida_float_mold_int",
    "taida_float_mold_str",
    "taida_float_neg",
    "taida_float_pow",
    "taida_float_round",
    "taida_float_sin",
    "taida_float_sinh",
    "taida_float_sqrt",
    "taida_float_tan",
    "taida_float_tanh",
    "taida_float_to_fixed",
    "taida_float_to_int",
    "taida_float_to_str",
    "taida_generic_unmold",
    "taida_int_abs",
    "taida_int_mold_auto",
    "taida_int_mold_bool",
    "taida_int_mold_float",
    "taida_int_mold_int",
    "taida_int_mold_str",
    "taida_int_mold_str_base",
    "taida_int_to_float",
    "taida_int_to_str",
    "taida_monadic_to_string",
    "taida_slice_mold",
    "taida_str_char_at",
    "taida_str_mold_any",
    "taida_str_mold_bool",
    "taida_str_mold_float",
    "taida_str_mold_int",
    "taida_str_mold_str",
    "taida_str_to_int",
];

/// Deliberately divergent: JSON parse/encode shares grammar but not
/// number formatting or buffer management (libc strtod/snprintf vs
/// manual strtod/ftoa).
const DIVERGENT_JSON: &[&str] = &[
    "taida_json_empty",
    "taida_json_encode",
    "taida_json_from_int",
    "taida_json_from_str",
    "taida_json_has",
    "taida_json_parse",
    "taida_json_pretty",
    "taida_json_schema_cast",
    "taida_json_size",
    "taida_json_to_int",
    "taida_json_unmold",
];

/// Deliberately divergent: the error ceiling rides setjmp/longjmp on
/// native; WASM has no setjmp and uses its own unwind strategy, and the
/// Lax/Result/Gorillax constructors sit on the divergent allocators.
const DIVERGENT_ERROR_CEILING: &[&str] = &[
    "taida_error_ceiling_pop",
    "taida_error_ceiling_push",
    "taida_error_get_value",
    "taida_error_info",
    "taida_error_setjmp",
    "taida_error_try_call",
    "taida_error_try_get_result",
    "taida_error_type_matches",
    "taida_gorilla",
    "taida_gorillax_err",
    "taida_gorillax_new",
    "taida_gorillax_relax",
    "taida_gorillax_to_string",
    "taida_gorillax_unmold",
    "taida_lax_empty",
    "taida_lax_empty_error",
    "taida_lax_flat_map",
    "taida_lax_map",
    "taida_lax_new",
    "taida_lax_to_string",
    "taida_lax_value_ekind",
    "taida_relaxed_gorillax_to_string",
    "taida_relaxed_gorillax_unmold",
    "taida_result_create",
    "taida_result_flat_map",
    "taida_result_get_or_default",
    "taida_result_get_or_throw",
    "taida_result_is_error",
    "taida_result_is_error_check",
    "taida_result_is_ok",
    "taida_result_map",
    "taida_result_map_error",
    "taida_result_to_string",
    "taida_throw",
];

/// Deliberately divergent: remaining shared surface whose bodies lean
/// on target-specific helpers (heap probing, value-tag stacks, regex
/// availability, string interning) without forming a bigger theme.
const DIVERGENT_MISC: &[&str] = &[
    "taida_can_throw_payload",
    "taida_collection_get",
    "taida_collection_has",
    "taida_collection_has_tagged",
    "taida_collection_remove",
    "taida_collection_remove_tagged",
    "taida_collection_size",
    "taida_detect_gorillax_type",
    "taida_detect_value_tag",
    "taida_get_call_arg_tag",
    "taida_get_return_tag",
    "taida_has_magic_header",
    "taida_invoke_callback1",
    "taida_invoke_callback2",
    "taida_lookup_field_name",
    "taida_lookup_field_type",
    "taida_make_error",
    "taida_make_error_with_kind",
    "taida_make_error_with_kind_code",
    "taida_make_io_error",
    "taida_molten_new",
    // F56: opaque secret carriers + redaction. native (taida_val) / wasm
    // (int64_t) signatures normalize to identical bodies (same pack shape).
    "taida_moltenize_new",
    "taida_secret_new",
    "taida_redact",
    "taida_monadic_field_count",
    "taida_monadic_flat_map",
    "taida_monadic_get_or_throw",
    "taida_pop_call_tags",
    "taida_primitive_tag_match",
    "taida_ptr_is_readable",
    "taida_push_call_tags",
    "taida_read_cstr_len_safe",
    "taida_regex_new",
    "taida_register_field_name",
    "taida_register_field_type",
    "taida_register_type_parent",
    "taida_sha256",
    "taida_str_contains",
    "taida_str_ends_with",
    "taida_str_eq",
    "taida_str_get",
    "taida_str_hash",
    "taida_str_index_of",
    "taida_str_last_index_of",
    "taida_str_length",
    "taida_str_match_regex",
    "taida_str_neq",
    "taida_str_pad",
    "taida_str_release",
    "taida_str_retain",
    "taida_str_reverse",
    "taida_str_search_regex",
    "taida_str_search_regex_lax",
    "taida_str_starts_with",
    "taida_str_to_lower",
    "taida_str_to_upper",
    "taida_stream_new",
    "taida_stub_new",
    "taida_to_radix",
    "taida_todo_new",
    "taida_type_name",
    "taida_typeis_named",
    "taida_value_hash",
    "taida_value_to_debug_string",
    "taida_value_to_display_string",
];

/// All divergence groups with their reasons (the group docs above hold
/// the long form; the strings here surface in test failure messages).
pub(crate) const DIVERGENT_GROUPS: &[(&str, &[&str])] = &[
    (
        "iter-scope watermark: chunked-arena mark vs bump-pointer wrapper",
        DIVERGENT_ITER_SCOPE,
    ),
    ("async: pthread vs deterministic stubs", DIVERGENT_ASYNC),
    (
        "io/debug: libc stdio vs fd_write imports",
        DIVERGENT_IO_DEBUG,
    ),
    (
        "crypto: shared algorithms, different buffer plumbing",
        DIVERGENT_CRYPTO,
    ),
    (
        "containers: freelist+arena+rc vs bump+no-op rc",
        DIVERGENT_CONTAINER_ALLOC_RC,
    ),
    (
        "poly/type-detect: target-specific heap probing",
        DIVERGENT_POLY_STR_DETECT,
    ),
    (
        "mold/float-math: libm+printf vs portable reimpl",
        DIVERGENT_MOLD_CONV,
    ),
    ("json: libc number formatting vs manual", DIVERGENT_JSON),
    (
        "error ceiling: setjmp/longjmp vs wasm unwind",
        DIVERGENT_ERROR_CEILING,
    ),
    ("misc: target-specific helper dependencies", DIVERGENT_MISC),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::native_runtime::NATIVE_RUNTIME_C;
    use crate::codegen::runtime_core_wasm::RUNTIME_CORE_WASM;
    use std::collections::BTreeSet;

    /// The extractor must not be fooled by C constructs that put
    /// signature-looking text or braces inside comments, strings, or
    /// char literals, and must skip prototypes and nested scopes.
    #[test]
    fn extractor_survives_adversarial_c() {
        let src = r#"
// taida_fake_line(int64_t a) { return a; }
/* taida_fake_block(int64_t a) { return a; } */
static const char *fake = "taida_fake_string(int64_t a) { return a; }";
int64_t taida_real_fn(int64_t a) {
    const char *brace_str = "}";
    char brace_chr = '}';
    // stray } in comment
    if (a) { return 1; }
    return 0;
}
int64_t taida_proto_only(int64_t a);
void helper_caller(void) {
    int64_t taida_not_a_def = 0;
    (void)taida_not_a_def;
}
static char* taida_star_attached(int64_t a) { return 0; }
static const char *taida_star_spaced(int64_t a) { return 0; }
"#;
        let defs = enumerate_fn_defs(src);
        assert!(defs.contains_key("taida_real_fn"));
        assert!(defs.contains_key("helper_caller"));
        assert!(!defs.contains_key("taida_fake_line"));
        assert!(!defs.contains_key("taida_fake_block"));
        assert!(!defs.contains_key("taida_fake_string"));
        assert!(!defs.contains_key("taida_proto_only"));
        assert!(!defs.contains_key("taida_not_a_def"));
        // pointer return types parse in both spellings (`char*` / `char *`)
        assert!(defs.contains_key("taida_star_attached"));
        assert!(defs.contains_key("taida_star_spaced"));
        let body = &defs["taida_real_fn"];
        assert!(body.contains("return 0;"), "body cut short: {body}");
        assert!(body.trim_end().ends_with('}'));
    }

    /// Normalization tolerates exactly the value-typedef spelling,
    /// comments, and whitespace layout — nothing else.
    #[test]
    fn normalization_tolerates_only_typename_comments_whitespace() {
        let native =
            "taida_val taida_x(taida_val a)  {\n    // doc style\n    return a; /* note */\n}";
        let wasm = "int64_t taida_x(int64_t a) { return a; }";
        assert_eq!(normalize_def(native), normalize_def(wasm));
        // a real change must NOT normalize away
        let drifted = "int64_t taida_x(int64_t a) { return a + 1; }";
        assert_ne!(normalize_def(native), normalize_def(drifted));
        // identifiers merely containing the typedef name are untouched
        assert_eq!(
            normalize_def("int taida_val_count;"),
            "int taida_val_count;"
        );
        // comment-looking sequences inside string literals are code
        assert_ne!(
            normalize_def(r#"int64_t f() { return s("//x"); }"#),
            normalize_def(r#"int64_t f() { return s(""); }"#)
        );
        // the typedef rename never rewrites string literal contents —
        // a message drift must stay visible
        assert_ne!(
            normalize_def(r#"int64_t f() { return s("taida_val"); }"#),
            normalize_def(r#"int64_t f() { return s("int64_t"); }"#)
        );
    }

    /// Every `taida_*` symbol defined in BOTH runtimes must be
    /// classified: identical-by-contract (allowlist) or deliberately
    /// divergent (a divergence group) — never neither, never both, and
    /// never stale.
    #[test]
    fn every_common_symbol_is_classified() {
        let native = enumerate_fn_defs(&NATIVE_RUNTIME_C);
        let wasm = enumerate_fn_defs(&RUNTIME_CORE_WASM);
        let common: BTreeSet<&str> = native
            .keys()
            .filter(|k| k.starts_with("taida_") && wasm.contains_key(*k))
            .map(|s| s.as_str())
            .collect();

        let allow: BTreeSet<&str> = MIRROR_SYNC_ALLOWLIST.iter().copied().collect();
        let mut divergent: BTreeSet<&str> = BTreeSet::new();
        for (reason, group) in DIVERGENT_GROUPS {
            for name in *group {
                assert!(
                    divergent.insert(name),
                    "{name} listed in more than one divergence group (second: {reason})"
                );
            }
        }

        for name in &allow {
            assert!(
                !divergent.contains(name),
                "{name} is in both the allowlist and a divergence group"
            );
        }
        let unclassified: Vec<&&str> = common
            .iter()
            .filter(|n| !allow.contains(*n) && !divergent.contains(*n))
            .collect();
        assert!(
            unclassified.is_empty(),
            "unclassified common runtime symbols {unclassified:?}: add each \
             to MIRROR_SYNC_ALLOWLIST (bodies must stay identical) or to a \
             DIVERGENT_* group (with the divergence reason)"
        );
        let stale: Vec<&&str> = allow
            .iter()
            .chain(divergent.iter())
            .filter(|n| !common.contains(*n))
            .collect();
        assert!(
            stale.is_empty(),
            "stale classification entries {stale:?}: no longer defined in \
             both runtimes — remove them from the table"
        );
        println!(
            "mirror-sync metrics: common={} allowlist={} helpers={} divergent={}",
            common.len(),
            MIRROR_SYNC_ALLOWLIST.len(),
            HELPER_ALLOWLIST.len(),
            divergent.len()
        );
    }

    /// The gate itself: allowlisted bodies (and the non-taida helpers
    /// they call) are identical across runtimes after normalization. A
    /// failure here usually means an edit landed on one runtime only —
    /// finish it on the other side; demote to a divergence group only
    /// as a recorded design decision.
    #[test]
    fn allowlisted_bodies_are_identical_across_runtimes() {
        let native = enumerate_fn_defs(&NATIVE_RUNTIME_C);
        let wasm = enumerate_fn_defs(&RUNTIME_CORE_WASM);
        for name in MIRROR_SYNC_ALLOWLIST.iter().chain(HELPER_ALLOWLIST) {
            let n = native
                .get(*name)
                .unwrap_or_else(|| panic!("{name} missing from the native runtime"));
            let w = wasm
                .get(*name)
                .unwrap_or_else(|| panic!("{name} missing from the wasm runtime"));
            assert_eq!(
                normalize_def(n),
                normalize_def(w),
                "mirror drift in `{name}`: the native and wasm definitions \
                 differ beyond the value-typedef spelling — finish the edit \
                 on both runtimes (or reclassify with a divergence reason)"
            );
        }
    }
}
