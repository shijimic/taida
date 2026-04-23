//! C12 Phase 6 (FB-5 Phase 2-3) — Regex type support.
//!
//! The Taida `:Regex` type is represented as a typed BuchiPack with
//! `__type <= "Regex"`, `pattern <= Str`, `flags <= Str`. The constructor
//! lives in [`crate::interpreter::prelude`] as the prelude function
//! `Regex(pattern, flags?)`. Str method overloads (`replace`, `replaceAll`,
//! `split`, `match`, `search`) detect a Regex BuchiPack as the first arg
//! and dispatch through this module.
//!
//! **Philosophy alignment**:
//! * I. No `null`/`undefined`: construction with an invalid pattern yields
//!   an `Error` BuchiPack (via `RuntimeError::Throw`), never a silent
//!   undefined state.
//! * II. "Bag" form: `Regex(...)` is a BuchiPack, introspectable via
//!   `typeof(r) == "Regex"` and `r.pattern` / `r.flags`.
//! * No implicit coercion: Str methods dispatch explicitly by detecting
//!   the BuchiPack `__type` tag; everything else is a literal string match.
//!
//! **Escape semantics** (design lock `C12_DESIGN.md` §C12-6):
//! * `\d` / `\w` / `\s` / `\b` / `\D` / `\W` / `\S` / `\B`
//! * `\x{HH}` / `\u{HHHH}` (via Rust `regex` crate defaults)
//! * JS `$&` / `$$` / `$1` meta-syntax in the replacement string is
//!   disabled: replacements are applied literally.
//!
//! **Flags**: `i` (case-insensitive), `m` (multiline `^`/`$`), `s`
//! (dotall). `g` is implicit — `replace` replaces first, `replaceAll`
//! replaces all. Unknown flag chars produce a validation error at
//! `Regex(...)` construction time.

use super::value::Value;
use regex::{Regex, RegexBuilder};
use std::cell::RefCell;
use std::collections::HashMap;

/// Internal tag marker stored in `Value::BuchiPack` `__type` for
/// Regex values.
pub(crate) const REGEX_TYPE_TAG: &str = "Regex";

/// C12B-036 / C25B-024: per-thread cache for compiled regex objects.
///
/// Each Str-method overload (`replace`, `replaceAll`, `split`, `match`,
/// `search`) and `build_regex_value` used to call [`compile`] — which
/// allocates a new `Regex` from scratch every time. In a hot loop
/// (`values => map(v => v.replace(Regex("..."), "..."))`) this means
/// the same pattern is re-parsed on every iteration. The cache below
/// stashes the most recently used `(pattern, flags)` pairs so that
/// subsequent calls return the shared `Regex` without any re-parsing.
///
/// Notes:
/// * Thread-local: `Regex` is `Sync + Send` but we keep the cache
///   private per thread to avoid locking on the hot path. The worst
///   case is a small per-thread memory footprint.
/// * **C25B-024 migration (2026-04-23)**: the C12B-036 VecDeque-based
///   FIFO cache was replaced with a `HashMap<(String, String), Regex>`
///   plus an eviction-order `VecDeque<(String, String)>` to keep
///   lookups at O(1) while preserving the fixed capacity of 64.
///   Previous behaviour walked the entire VecDeque on every cache
///   lookup, which dominated regex-heavy loops (lexers, tokenisers,
///   template substitution). Per-lookup cost drops from O(capacity)
///   to O(1). FIFO semantics are preserved so saturation behaviour
///   is identical.
/// * Capacity 64 mirrors the JS runtime's `__TAIDA_REGEX_CACHE_CAPACITY`
///   so the three backends behave similarly under memory pressure.
/// * Invalid patterns are never cached: [`compile`] returns `Err`
///   before reaching [`cached_compile`].
const REGEX_CACHE_CAPACITY: usize = 64;

thread_local! {
    /// The compiled regex table, keyed on (pattern, flags).
    static REGEX_CACHE: RefCell<HashMap<(String, String), Regex>> =
        RefCell::new(HashMap::with_capacity(REGEX_CACHE_CAPACITY));
    /// FIFO eviction order — the key at the front is the next to evict
    /// when the table reaches capacity.
    static REGEX_CACHE_ORDER: RefCell<std::collections::VecDeque<(String, String)>> =
        RefCell::new(std::collections::VecDeque::with_capacity(REGEX_CACHE_CAPACITY));
}

/// Return a compiled `Regex` for `(pattern, flags)` from the thread-local
/// cache, inserting it on a miss. Cloning a `Regex` is cheap — internally
/// it is an `Arc`-shared pointer to the NFA, so the returned value is a
/// true share rather than a fresh compile.
fn cached_compile(pattern: &str, flags: &str) -> Result<Regex, String> {
    let key = (pattern.to_string(), flags.to_string());
    // Fast O(1) hash lookup.
    let hit = REGEX_CACHE.with(|cell| cell.borrow().get(&key).cloned());
    if let Some(re) = hit {
        return Ok(re);
    }
    let re = compile(pattern, flags)?;
    REGEX_CACHE.with(|cell| {
        REGEX_CACHE_ORDER.with(|order_cell| {
            let mut map = cell.borrow_mut();
            let mut order = order_cell.borrow_mut();
            if map.len() >= REGEX_CACHE_CAPACITY
                && let Some(oldest) = order.pop_front()
            {
                map.remove(&oldest);
            }
            if !map.contains_key(&key) {
                order.push_back(key.clone());
            }
            map.insert(key, re.clone());
        });
    });
    Ok(re)
}

/// Detect whether a [`Value`] is a Regex BuchiPack. Returns the
/// `(pattern, flags)` tuple when so.
pub(crate) fn as_regex(val: &Value) -> Option<(String, String)> {
    let Value::BuchiPack(fields) = val else {
        return None;
    };
    let is_regex = fields
        .iter()
        .any(|(k, v)| k == "__type" && matches!(v, Value::Str(s) if s == REGEX_TYPE_TAG));
    if !is_regex {
        return None;
    }
    let pattern = fields
        .iter()
        .find(|(k, _)| k == "pattern")
        .and_then(|(_, v)| match v {
            Value::Str(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let flags = fields
        .iter()
        .find(|(k, _)| k == "flags")
        .and_then(|(_, v)| match v {
            Value::Str(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default();
    Some((pattern, flags))
}

/// Build a Regex BuchiPack value. Pattern / flags are validated by
/// attempting to compile the regex eagerly; failure is surfaced as
/// `Err(message)` for the prelude caller to translate into a throwable
/// `:Error`.
pub(crate) fn build_regex_value(pattern: &str, flags: &str) -> Result<Value, String> {
    validate_flags(flags)?;
    // Compile once at construction time to surface pattern errors eagerly
    // (no silent undefined state, per PHILOSOPHY I). The pre-compiled
    // `Regex` is also kept in the thread-local cache (`cached_compile`)
    // so subsequent Str-method calls on this pattern hit the cache on
    // the first use — a small but predictable warm-up, matching the JS
    // runtime where `new RegExp(...)` at construction primes the
    // `__taida_regex_cache` on the first method call.
    cached_compile(pattern, flags)?;
    Ok(Value::BuchiPack(vec![
        ("pattern".into(), Value::Str(pattern.to_string())),
        ("flags".into(), Value::Str(flags.to_string())),
        ("__type".into(), Value::Str(REGEX_TYPE_TAG.into())),
    ]))
}

fn validate_flags(flags: &str) -> Result<(), String> {
    for c in flags.chars() {
        match c {
            'i' | 'm' | 's' => {}
            other => {
                return Err(format!(
                    "Regex: unsupported flag '{}'. Supported flags: i (case-insensitive), m (multiline), s (dotall)",
                    other
                ));
            }
        }
    }
    Ok(())
}

fn compile(pattern: &str, flags: &str) -> Result<Regex, String> {
    let mut builder = RegexBuilder::new(pattern);
    builder.case_insensitive(flags.contains('i'));
    builder.multi_line(flags.contains('m'));
    builder.dot_matches_new_line(flags.contains('s'));
    builder
        .build()
        .map_err(|e| format!("Regex: invalid pattern '{}' — {}", pattern, e))
}

/// Apply `replace` (first match only) using the Regex semantics. The
/// replacement string is treated as a literal — no `$1` / `$&`
/// expansion — to match JS / Native parity (see design lock §C12-6).
pub(crate) fn replace_first(
    s: &str,
    pattern: &str,
    flags: &str,
    replacement: &str,
) -> Result<String, String> {
    let re = cached_compile(pattern, flags)?;
    // `regex::replacen` expands `$N` by default. Use `NoExpand` to
    // apply the replacement as a literal, matching the B11 Phase 1
    // fixed-string contract and JS's documented `$&` lock-down.
    Ok(re.replacen(s, 1, regex::NoExpand(replacement)).into_owned())
}

/// Apply `replaceAll` using the Regex semantics. Literal replacement —
/// no `$N` expansion.
pub(crate) fn replace_all(
    s: &str,
    pattern: &str,
    flags: &str,
    replacement: &str,
) -> Result<String, String> {
    let re = cached_compile(pattern, flags)?;
    Ok(re.replace_all(s, regex::NoExpand(replacement)).into_owned())
}

/// Apply `split` using the Regex semantics. Empty pattern is an error
/// (philosophy: no silent fallback — the user must explicitly call
/// `.split("")` for codepoint-split, which is the fixed-string overload).
pub(crate) fn split(s: &str, pattern: &str, flags: &str) -> Result<Vec<String>, String> {
    let re = cached_compile(pattern, flags)?;
    Ok(re.split(s).map(|part| part.to_string()).collect())
}

/// Single match result: `(start_byte, full_match, groups...)`.
/// Groups are 1-indexed; missing / non-participating groups become `""`.
pub(crate) struct MatchResult {
    pub start: i64,
    pub full: String,
    pub groups: Vec<String>,
}

/// Find the first regex match in `s`. Returns `Ok(None)` when no
/// match; `Err` only for invalid patterns (which should not happen
/// since `build_regex_value` pre-validated).
pub(crate) fn match_first(
    s: &str,
    pattern: &str,
    flags: &str,
) -> Result<Option<MatchResult>, String> {
    let re = cached_compile(pattern, flags)?;
    let Some(caps) = re.captures(s) else {
        return Ok(None);
    };
    let full_match = caps.get(0).expect("group 0 is always present on a match");
    let start_byte = full_match.start();
    // Convert byte offset to char offset for surface-level consistency
    // with `indexOf` / `length` (Taida's public Str API is char-based).
    let start_char = s[..start_byte].chars().count() as i64;
    let full = full_match.as_str().to_string();
    let groups: Vec<String> = (1..caps.len())
        .map(|i| {
            caps.get(i)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default()
        })
        .collect();
    Ok(Some(MatchResult {
        start: start_char,
        full,
        groups,
    }))
}

/// Char-based start index of the first match, or `-1` when no match.
/// Exposed as the `search` method's return value (`Int`).
pub(crate) fn search_first(s: &str, pattern: &str, flags: &str) -> Result<i64, String> {
    match match_first(s, pattern, flags)? {
        Some(m) => Ok(m.start),
        None => Ok(-1),
    }
}

/// Build the `:RegexMatch` BuchiPack that `str.match(Regex(...))`
/// returns. Uses `hasValue` / `__type <= "RegexMatch"` so that
/// `result.hasValue` is queriable via the existing Lax-like idiom
/// while keeping the payload (`full`, `groups`, `start`) inspectable.
pub(crate) fn build_match_value(m: Option<MatchResult>) -> Value {
    match m {
        Some(m) => Value::BuchiPack(vec![
            ("hasValue".into(), Value::Bool(true)),
            ("full".into(), Value::Str(m.full)),
            (
                "groups".into(),
                Value::list(m.groups.into_iter().map(Value::Str).collect()),
            ),
            ("start".into(), Value::Int(m.start)),
            ("__type".into(), Value::Str("RegexMatch".into())),
        ]),
        None => Value::BuchiPack(vec![
            ("hasValue".into(), Value::Bool(false)),
            ("full".into(), Value::Str(String::new())),
            ("groups".into(), Value::list(Vec::new())),
            ("start".into(), Value::Int(-1)),
            ("__type".into(), Value::Str("RegexMatch".into())),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_regex_value_roundtrip() {
        let v = build_regex_value("\\d+", "i").expect("valid pattern");
        let (p, f) = as_regex(&v).expect("detected as regex");
        assert_eq!(p, "\\d+");
        assert_eq!(f, "i");
    }

    #[test]
    fn test_build_regex_rejects_bad_flag() {
        let err = build_regex_value("a", "x").unwrap_err();
        assert!(err.contains("unsupported flag"));
    }

    #[test]
    fn test_build_regex_rejects_invalid_pattern() {
        let err = build_regex_value("(unclosed", "").unwrap_err();
        assert!(err.contains("invalid pattern"));
    }

    #[test]
    fn test_replace_first_literal_replacement_ignores_dollar_meta() {
        // `$&` / `$1` must be treated literally (JS parity note in the
        // design lock).
        let out = replace_first("abc", "b", "", "$&").expect("replace");
        assert_eq!(out, "a$&c");
    }

    #[test]
    fn test_replace_all_matches_pattern() {
        let out = replace_all("hello world", "[aeiou]", "", "*").expect("replace_all");
        assert_eq!(out, "h*ll* w*rld");
    }

    #[test]
    fn test_split_on_pattern() {
        let parts = split("a1b22c333d", "\\d+", "").expect("split");
        assert_eq!(parts, vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn test_match_first_extracts_groups() {
        let m = match_first("id: 12-34 other", "(\\d+)-(\\d+)", "")
            .expect("match ok")
            .expect("matched");
        assert_eq!(m.full, "12-34");
        assert_eq!(m.groups, vec!["12".to_string(), "34".to_string()]);
        assert_eq!(m.start, 4);
    }

    #[test]
    fn test_match_first_no_match() {
        let m = match_first("nothing", "\\d+", "").expect("match ok");
        assert!(m.is_none());
    }

    #[test]
    fn test_search_first_returns_char_index() {
        // `あ` is 3 UTF-8 bytes but 1 char, so the `\d` match at char
        // index 1 must be reported as 1, not as a byte offset.
        let idx = search_first("あ123", "\\d", "").expect("search");
        assert_eq!(idx, 1);
    }

    #[test]
    fn test_search_first_no_match() {
        let idx = search_first("abc", "\\d", "").expect("search");
        assert_eq!(idx, -1);
    }

    #[test]
    fn test_flag_i_case_insensitive() {
        let out = replace_all("AbC", "b", "i", "*").expect("replace_all");
        assert_eq!(out, "A*C");
    }

    #[test]
    fn test_flag_m_multiline_anchors() {
        let parts = split("ab\ncd", "^", "m").expect("split");
        // Each line start becomes a split boundary, producing an empty
        // leading string and each subsequent line as a separate piece.
        assert_eq!(parts, vec!["", "ab\n", "cd"]);
    }

    #[test]
    fn test_flag_s_dotall() {
        let m = match_first("a\nb", ".", "s")
            .expect("match")
            .expect("matched");
        assert_eq!(m.full, "a"); // first char is still first
        let m2 = match_first("\nb", ".", "s")
            .expect("match")
            .expect("matched");
        assert_eq!(m2.full, "\n"); // `.` now crosses newline
    }

    #[test]
    fn test_as_regex_rejects_non_regex_buchipack() {
        let lax = Value::BuchiPack(vec![
            ("hasValue".into(), Value::Bool(true)),
            ("__type".into(), Value::Str("Lax".into())),
        ]);
        assert!(as_regex(&lax).is_none());
    }

    #[test]
    fn test_build_match_value_some() {
        let v = build_match_value(Some(MatchResult {
            start: 2,
            full: "xy".into(),
            groups: vec!["x".into(), "y".into()],
        }));
        if let Value::BuchiPack(fields) = v {
            assert!(
                fields
                    .iter()
                    .any(|(k, v)| k == "hasValue" && matches!(v, Value::Bool(true)))
            );
            assert!(
                fields
                    .iter()
                    .any(|(k, v)| k == "__type" && matches!(v, Value::Str(s) if s == "RegexMatch"))
            );
        } else {
            panic!("expected BuchiPack");
        }
    }

    #[test]
    fn test_build_match_value_none() {
        let v = build_match_value(None);
        if let Value::BuchiPack(fields) = v {
            assert!(
                fields
                    .iter()
                    .any(|(k, v)| k == "hasValue" && matches!(v, Value::Bool(false)))
            );
            assert!(
                fields
                    .iter()
                    .any(|(k, v)| k == "start" && matches!(v, Value::Int(-1)))
            );
        } else {
            panic!("expected BuchiPack");
        }
    }

    // C12B-036: regex cache correctness. The FIFO cache must return
    // *semantically identical* Regex objects on repeated calls and must
    // not mask invalid-pattern failures.

    #[test]
    fn test_c12b_036_cached_compile_hits_return_same_regex() {
        // Two successive calls with the same (pattern, flags) should
        // both succeed and behave identically. We exercise the cache
        // by compiling twice and asserting the same match result.
        let re1 = cached_compile("\\d+", "").expect("first compile");
        let re2 = cached_compile("\\d+", "").expect("second compile (cache hit)");
        // Both share the underlying NFA via Arc; their `.as_str()` must
        // match and both must find the same match on the same input.
        assert_eq!(re1.as_str(), re2.as_str());
        assert_eq!(
            re1.find("abc123").map(|m| m.as_str()),
            re2.find("abc123").map(|m| m.as_str())
        );
    }

    #[test]
    fn test_c12b_036_cached_compile_distinguishes_flags() {
        // `a` with and without `i` must produce different case-sensitivity;
        // the cache key includes flags so they do not collide.
        let plain = cached_compile("a", "").expect("plain");
        let ci = cached_compile("a", "i").expect("ci");
        assert!(plain.is_match("a"));
        assert!(!plain.is_match("A"));
        assert!(ci.is_match("A"));
    }

    #[test]
    fn test_c12b_036_cached_compile_rejects_invalid_pattern() {
        // Invalid patterns must *not* be cached — every call surfaces
        // the error. The cache lookup is keyed by `(pattern, flags)`
        // so an invalid entry would remain unhit on subsequent calls,
        // but we still guarantee the error is returned both times.
        let err1 = cached_compile("(unclosed", "").unwrap_err();
        let err2 = cached_compile("(unclosed", "").unwrap_err();
        assert!(err1.contains("invalid pattern"));
        assert!(err2.contains("invalid pattern"));
    }

    #[test]
    fn test_c12b_036_cache_capacity_evicts_fifo() {
        // Fill the cache beyond capacity with distinct keys, then verify
        // the very first key is still retrievable as a compile (it was
        // evicted but re-compile works because the pattern is valid).
        // This is a behavioural test — we cannot directly inspect cache
        // contents without exposing it, so we rely on the invariant that
        // both cached and uncached paths produce identical Regex semantics.
        for i in 0..(REGEX_CACHE_CAPACITY + 10) {
            let p = format!("^{}$", i);
            let re = cached_compile(&p, "").expect("distinct pattern compiles");
            assert!(re.is_match(&i.to_string()));
        }
        // The first key (`^0$`) is no longer in the cache but must
        // still compile correctly.
        let re = cached_compile("^0$", "").expect("recompiles after eviction");
        assert!(re.is_match("0"));
    }
}
