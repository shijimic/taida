//! C12-1a: Mold return-type tag table — single source of truth.
//!
//! This module centralizes the mapping from builtin mold names to their
//! compile-time return type tags. Both `src/codegen/lower.rs`
//! (`expr_type_tag`) and `src/types/checker.rs` (builtin mold inference)
//! consume this table so that backend codegen and type checker never
//! disagree about a mold's return kind.
//!
//! Prior to C12-1 the tag table was inlined in `expr_type_tag()` and
//! returned `TAIDA_TAG_PACK` (4) for every `MoldInst`, which caused the
//! wasm runtime's polymorphic display dispatch to misclassify Str-
//! returning molds (Upper / Lower / Trim / Join / ...) as Pack/List
//! values. B11-2f worked around this by falling back to
//! `convert_to_string` at the call site; C12-1 eliminates that fallback
//! by making the tag table complete and authoritative.
//!
//! Tag values match the `TAIDA_TAG_*` constants in `native_runtime.c`
//! and the `WASM_TAG_*` constants in `runtime_core_wasm.c`:
//!
//! - `0` — Int
//! - `1` — Float
//! - `2` — Bool
//! - `3` — Str
//! - `4` — Pack
//! - `5` — List
//! - `6` — Closure
//!
//! A mold whose return tag depends on its arguments (e.g. `Slice`,
//! `Concat`, `Abs`, `Map`, `Filter`, `If`) returns `MoldReturnKind::Dynamic`
//! and callers fall back to argument-based inference or the UNKNOWN tag
//! (`-1`). User-defined molds (not in this table) always return Pack.
//!
//! See `.dev/C12_DESIGN.md` Workstream A and `.dev/FUTURE_BLOCKERS.md`
//! FB-27 for the design rationale.

/// Known return-type kind for a builtin mold.
///
/// `Dynamic` marks molds whose return type is determined by argument
/// types (e.g. `Concat`, `Slice`, `Abs`, `Map`). Such cases are resolved
/// at each call site; this table only fixes the cases that are constant
/// regardless of arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoldReturnKind {
    Int,
    Float,
    Bool,
    Str,
    Pack,
    List,
    /// Return type depends on arguments; caller must infer.
    Dynamic,
}

impl MoldReturnKind {
    /// Return the runtime type tag for this kind, or `None` if the kind
    /// is Dynamic (argument-dependent).
    pub fn tag(self) -> Option<i64> {
        match self {
            MoldReturnKind::Int => Some(0),
            MoldReturnKind::Float => Some(1),
            MoldReturnKind::Bool => Some(2),
            MoldReturnKind::Str => Some(3),
            MoldReturnKind::Pack => Some(4),
            MoldReturnKind::List => Some(5),
            MoldReturnKind::Dynamic => None,
        }
    }
}

/// Look up the return-type kind of a builtin mold by name.
///
/// Returns `None` for user-defined molds (they always return `Pack` in
/// codegen — callers should treat `None` as "unknown builtin" and apply
/// their usual fallback).
pub fn lookup_mold_return_kind(name: &str) -> Option<MoldReturnKind> {
    use MoldReturnKind::*;
    Some(match name {
        // ── Int-returning molds ─────────────────────────────────────
        "Int" => Int,
        "Length" | "Count" | "IndexOf" | "LastIndexOf" | "FindIndex" => Int,
        "Floor" | "Ceil" | "Round" | "Truncate" => Int,
        "BitAnd" | "BitOr" | "BitXor" | "BitNot" => Int,

        // ── Float-returning molds ───────────────────────────────────
        "Float" => Float,
        "Sqrt" | "Pow" => Float,

        // ── Bool-returning molds ────────────────────────────────────
        "Bool" => Bool,
        "TypeIs" | "TypeExtends" | "Exists" | "Contains" => Bool,

        // ── Str-returning molds ─────────────────────────────────────
        // NB: B11-2f previously routed these through `convert_to_string`
        // because `expr_type_tag` hardcoded Pack (4). C12-1 removes that
        // workaround; these must stay authoritative.
        "Str" => Str,
        "Upper" | "Lower" | "Trim" => Str,
        "Replace" | "ReplaceAll" => Str,
        "Repeat" => Str,
        "Pad" | "PadLeft" | "PadRight" => Str,
        "Join" => Str,
        "ToFixed" | "ToRadix" => Str,
        // CharAt returns `Lax[Str]` at the checker level (Pack at
        // runtime because Lax is a Pack). Treat as Pack for tag purposes.
        "CharAt" => Pack,

        // ── List-returning molds ────────────────────────────────────
        "Chars" | "Split" => List,
        "Sort" | "Unique" | "Flatten" => List,
        "Take" | "TakeWhile" | "Drop" | "DropWhile" => List,
        "Append" | "Prepend" | "Zip" | "Enumerate" => List,
        "BytesToList" => List,

        // ── Pack-returning molds (wrappers / result types) ──────────
        "Lax" | "Result" | "Async" | "Gorillax" | "RelaxedGorillax" | "Stream" | "StreamFrom" => {
            Pack
        }
        "Molten" | "Cage" => Pack,
        "Find" => Pack, // Lax[T]
        "ShiftL" | "ShiftR" | "ShiftRU" => Pack, // Lax[Int]
        "ByteSet" => Pack, // Lax[Bytes]

        // ── Dynamic (argument-dependent) ────────────────────────────
        // These molds' return kinds depend on argument types at the
        // call site; callers should fall back to their own inference.
        "Map" | "Filter" => Dynamic, // list or stream of transformed elems
        "Reverse" => Dynamic,         // Str or List
        "Concat" => Dynamic,          // Bytes / List / List[Unknown]
        "Slice" => Dynamic,           // Str or Bytes
        "Abs" | "Clamp" => Dynamic,   // Int / Float / Num
        "Sum" | "Min" | "Max" => Dynamic, // Int or Float depending on list elem
        "If" => Dynamic,              // type of then branch
        "Fold" | "Foldr" | "Reduce" => Dynamic, // accumulator type

        // Unknown (user-defined) — caller decides.
        _ => return None,
    })
}

/// Return the runtime tag for a builtin mold, if the return kind is
/// statically known (not Dynamic, not user-defined).
///
/// Used by `src/codegen/lower.rs::expr_type_tag()` to decide whether a
/// `MoldInst` can be dispatched through `taida_io_stdout_with_tag`
/// directly (no `convert_to_string` fallback needed).
pub fn mold_return_tag(name: &str) -> Option<i64> {
    lookup_mold_return_kind(name).and_then(|k| k.tag())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_returning_molds_map_to_str_tag() {
        for name in [
            "Str", "Upper", "Lower", "Trim", "Replace", "ReplaceAll", "Repeat", "Pad", "PadLeft",
            "PadRight", "Join", "ToFixed", "ToRadix",
        ] {
            assert_eq!(mold_return_tag(name), Some(3), "{name} should be Str (3)");
        }
    }

    #[test]
    fn bool_returning_molds_map_to_bool_tag() {
        for name in ["TypeIs", "TypeExtends", "Exists", "Contains", "Bool"] {
            assert_eq!(mold_return_tag(name), Some(2), "{name} should be Bool (2)");
        }
    }

    #[test]
    fn int_returning_molds_map_to_int_tag() {
        for name in [
            "Int",
            "Length",
            "Count",
            "IndexOf",
            "LastIndexOf",
            "FindIndex",
            "Floor",
            "Ceil",
            "Round",
            "Truncate",
            "BitAnd",
            "BitOr",
            "BitXor",
            "BitNot",
        ] {
            assert_eq!(mold_return_tag(name), Some(0), "{name} should be Int (0)");
        }
    }

    #[test]
    fn list_returning_molds_map_to_list_tag() {
        for name in [
            "Chars",
            "Split",
            "Sort",
            "Unique",
            "Flatten",
            "Take",
            "TakeWhile",
            "Drop",
            "DropWhile",
            "Append",
            "Prepend",
            "Zip",
            "Enumerate",
            "BytesToList",
        ] {
            assert_eq!(mold_return_tag(name), Some(5), "{name} should be List (5)");
        }
    }

    #[test]
    fn dynamic_molds_return_none_for_tag() {
        for name in [
            "Map", "Filter", "Reverse", "Concat", "Slice", "Abs", "Clamp", "Sum", "Min", "Max",
            "If", "Fold", "Foldr", "Reduce",
        ] {
            assert_eq!(
                mold_return_tag(name),
                None,
                "{name} is argument-dependent"
            );
        }
    }

    #[test]
    fn user_defined_molds_return_none() {
        assert_eq!(mold_return_tag("MyMold"), None);
        assert_eq!(mold_return_tag("UserPack"), None);
    }

    #[test]
    fn pack_returning_molds_map_to_pack_tag() {
        for name in [
            "Lax", "Result", "Async", "Gorillax", "Stream", "StreamFrom", "Molten", "Cage",
            "Find", "CharAt", "ShiftL", "ShiftR", "ByteSet",
        ] {
            assert_eq!(mold_return_tag(name), Some(4), "{name} should be Pack (4)");
        }
    }
}
