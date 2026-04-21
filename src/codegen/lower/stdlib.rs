// C12B-024: src/codegen/lower.rs mechanical split (FB-21 / C12-9 Step 2).
// C13-2: Further mechanical split — `taida-lang/net` surface moved to
// `lower/net.rs`, `taida-lang/os` + `taida-lang/pool` surfaces moved
// to `lower/os.rs`. This file retains only the stdlib IO bridge
// (`stdout` / `stderr` / `stdin`), the field-tag registry helper, and
// the crypto mapping. All behaviour is preserved; only the enclosing
// file changes.

use super::Lowering;

impl Lowering {
    /// stdout/stderr/stdin → C ランタイム関数名にマッピング (prelude builtins)
    pub(super) fn stdlib_io_mapping(sym: &str) -> Option<&'static str> {
        match sym {
            "stdout" => Some("taida_io_stdout"),
            "stderr" => Some("taida_io_stderr"),
            "stdin" => Some("taida_io_stdin"),
            // C20-2: UTF-8-aware line editor. Returns Async[Lax[Str]].
            "stdinLine" => Some("taida_io_stdin_line"),
            _ => None,
        }
    }

    /// Register a field type tag, detecting conflicts.
    /// If the same field name is used with different types across different
    /// BuchiPack/Mold definitions (e.g., `Todo.status: Str` vs `HttpResp.status: Int`),
    /// the tag is set to 0 (unknown) so the JSON serializer falls back to
    /// runtime heuristic type detection instead of using the wrong type.
    pub(super) fn register_field_type_tag(&mut self, name: &str, tag: i64) {
        if tag == 0 {
            return;
        }
        if let Some(&existing) = self.field_type_tags.get(name) {
            if existing != tag && existing != 0 {
                // Conflict: same field name used with different types.
                // Set to 0 (unknown) to force runtime heuristic detection.
                self.field_type_tags.insert(name.to_string(), 0);
            }
            // If existing == tag, no change needed (same type, idempotent).
            // If existing == 0, already conflicted, leave it.
        } else {
            self.field_type_tags.insert(name.to_string(), tag);
        }
    }

    /// taida-lang/crypto package function → C runtime function mapping.
    pub(super) fn crypto_func_mapping(sym: &str) -> Option<&'static str> {
        match sym {
            "sha256" => Some("taida_sha256"),
            _ => None,
        }
    }
}
