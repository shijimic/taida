use cranelift_codegen::ir::types;
/// Taida IR → CLIF IR 変換（Cranelift Emission）
use cranelift_codegen::ir::{self as clif, AbiParam, InstBuilder};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::HashMap;
use target_lexicon::Triple;

use super::ir::*;

// ---------------------------------------------------------------------------
// W-0a: コンパイルターゲットと ABI ヘルパー
// ---------------------------------------------------------------------------

/// コンパイルターゲット
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CompileTarget {
    /// ホストネイティブ（x86_64/aarch64, SystemV）
    Native,
    /// WASM32（wasm32-unknown-unknown, WasmBasicCAbi）
    Wasm32,
}

/// ターゲット依存の型ヘルパー
///
/// W-0 では `CompileTarget::Native` のみ。全メソッドが I64 を返すため
/// 出力は一切変わらない。コード上で「内部 boxed value」と「runtime ABI 上の
/// pointer/fn pointer」の意味が明示される。
#[derive(Clone, Copy)]
struct AbiHelper {
    target: CompileTarget,
}

impl AbiHelper {
    fn new(target: CompileTarget) -> Self {
        Self { target }
    }

    /// Taida の内部 boxed value 型。
    /// user function / IrVar / CallUser / CallIndirect の境界では常に I64 を維持する。
    fn value_ty(&self) -> clif::Type {
        types::I64
    }

    /// ランタイム関数 ABI 上のヒープポインタ型
    fn ptr_ty(&self) -> clif::Type {
        match self.target {
            CompileTarget::Native => types::I64,
            CompileTarget::Wasm32 => types::I32,
        }
    }

    /// ランタイム関数 ABI 上の関数ポインタ型
    fn fn_ptr_ty(&self) -> clif::Type {
        match self.target {
            CompileTarget::Native => types::I64,
            CompileTarget::Wasm32 => types::I32,
        }
    }

    fn triple(&self) -> Triple {
        match self.target {
            CompileTarget::Native => Triple::host(),
            CompileTarget::Wasm32 => "wasm32-unknown-unknown".parse().unwrap(),
        }
    }

    fn call_conv(&self) -> CallConv {
        // Cranelift 0.129.1 では WASM 用の専用 CallConv がないため、
        // triple_default() に委譲する。wasm32 triple でも SystemV が返される
        // （Cranelift が内部で WASM ABI を適用する）。
        CallConv::triple_default(&self.triple())
    }
}

// ---------------------------------------------------------------------------
// W-0b: ABI 種別テーブル — runtime 関数の引数/戻り値を意味的に分類する
// ---------------------------------------------------------------------------

/// 引数/戻り値の ABI 種別
///
/// W-0 では `CompileTarget::Native` のみが有効であり、`resolve_abi()` は
/// 全ての AbiKind に対して `I64`（Val/Ptr/FnPtr）または `F64` を返す。
/// つまり生成コードは変わらないが、将来 Wasm32 ターゲットを追加した際に
/// Ptr/FnPtr が I32 になる箇所が明示される。
#[derive(Debug, Clone, Copy)]
enum AbiKind {
    /// 整数値 (Int, Bool, Hash, tag, count, index, dummy) → value_ty()
    Val,
    /// ヒープポインタ (Str, Pack, List, HashMap, Set, Async, Closure, Lax, Result, ...) → ptr_ty()
    Ptr,
    /// 関数ポインタ → fn_ptr_ty()
    FnPtr,
    /// Float（常に F64、ターゲット非依存）
    F64,
}

/// ランタイム関数の ABI 定義
struct RuntimeAbi {
    params: &'static [AbiKind],
    returns: &'static [AbiKind],
}

/// ランタイム関数名から ABI 定義を取得する
///
/// 全 120+ ランタイム関数を「引数/戻り値がポインタか値か」で分類。
/// 分類基準:
/// - Val: 数値・真偽値・ハッシュ・タグ・カウント・インデックス・ダミー戻り値
/// - Ptr: ヒープ確保オブジェクト（Str, Pack, List, HashMap, Set, Async, Closure,
///   Lax, Result, Gorillax, Bytes, JSON, Molten 等）
/// - FnPtr: 関数ポインタ（クロージャ生成・list_map/filter 等のコールバック）
/// - F64: 浮動小数点数（bitcast 経由で boxed value と変換される場合あり）
fn runtime_abi(name: &str) -> Result<RuntimeAbi, String> {
    use AbiKind::*;
    Ok(match name {
        // ── Debug 出力 ──
        "taida_debug_int" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_debug_float" => RuntimeAbi {
            params: &[F64],
            returns: &[Val],
        },
        "taida_debug_bool" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_debug_str" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_debug_polymorphic" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_debug_list" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_debug_json" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_gorilla" => RuntimeAbi {
            params: &[],
            returns: &[],
        },

        // ── 整数演算 ──
        "taida_int_add" | "taida_int_sub" | "taida_int_mul" | "taida_poly_add" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },
        "taida_div_mold" | "taida_mod_mold" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Ptr],
        },
        "taida_bit_and" | "taida_bit_or" | "taida_bit_xor" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },
        "taida_shift_l" | "taida_shift_r" | "taida_shift_ru" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },
        "taida_to_radix" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Ptr],
        },
        "taida_bit_not" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_int_neg" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_float_neg" => RuntimeAbi {
            params: &[F64],
            returns: &[F64],
        },

        // ── 比較演算 ──
        // Note: taida_str_eq/neq は Ptr 比較だが、boxed I64 として渡すため Val として扱う。
        // taida_poly_eq/neq は動的ディスパッチで型不明なため Val として扱う。
        "taida_int_eq" | "taida_int_neq" | "taida_str_eq" | "taida_str_neq" | "taida_poly_eq"
        | "taida_poly_neq" | "taida_int_lt" | "taida_int_gt" | "taida_int_gte" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },

        // ── ブール演算 ──
        "taida_bool_and" | "taida_bool_or" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },
        "taida_bool_not" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },

        // ── ぶちパック操作 ──
        "taida_pack_new" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_pack_set" => RuntimeAbi {
            params: &[Ptr, Val, Val],
            returns: &[Ptr],
        },
        "taida_pack_get" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Val],
        },
        "taida_pack_get_idx" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Val],
        },
        "taida_pack_set_hash" => RuntimeAbi {
            params: &[Ptr, Val, Val],
            returns: &[Ptr],
        },
        "taida_pack_set_tag" => RuntimeAbi {
            params: &[Ptr, Val, Val],
            returns: &[Ptr],
        },
        // NB-14: Stack-based call-site arg tag propagation (Bool/Int disambiguation)
        "taida_push_call_tags" => RuntimeAbi {
            params: &[],
            returns: &[],
        },
        "taida_pop_call_tags" => RuntimeAbi {
            params: &[],
            returns: &[],
        },
        "taida_set_call_arg_tag" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },
        "taida_get_call_arg_tag" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_set_return_tag" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_get_return_tag" => RuntimeAbi {
            params: &[],
            returns: &[Val],
        },
        // BuchiPack field call (polymorphic dispatch: args are boxed values)
        "taida_pack_call_field0" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Val],
        },
        "taida_pack_call_field1" => RuntimeAbi {
            params: &[Ptr, Val, Val],
            returns: &[Val],
        },
        "taida_pack_call_field2" => RuntimeAbi {
            params: &[Ptr, Val, Val, Val],
            returns: &[Val],
        },
        "taida_pack_call_field3" => RuntimeAbi {
            params: &[Ptr, Val, Val, Val, Val],
            returns: &[Val],
        },

        // ── クロージャ操作 ──
        "taida_closure_new" => RuntimeAbi {
            params: &[FnPtr, Ptr],
            returns: &[Ptr],
        },
        "taida_closure_get_fn" => RuntimeAbi {
            params: &[Ptr],
            returns: &[FnPtr],
        },
        "taida_closure_get_env" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_is_closure_value" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },

        // ── グローバル変数テーブル ──
        "taida_global_set" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[],
        },
        "taida_global_get" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },

        // ── リスト操作 ──
        "taida_list_new" => RuntimeAbi {
            params: &[],
            returns: &[Ptr],
        },
        "taida_list_set_elem_tag" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[],
        },
        "taida_list_push" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_list_length" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_list_get" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_list_first" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_list_last" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_list_sum" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_list_reverse" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_list_contains" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Val],
        },
        "taida_list_is_empty" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_list_map" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Ptr],
        },
        "taida_list_filter" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Ptr],
        },

        // ── 型変換 (Int) ──
        "taida_int_abs" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_int_to_str" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_int_to_float" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_int_clamp" => RuntimeAbi {
            params: &[Val, Val, Val],
            returns: &[Val],
        },

        // ── 文字列操作 ──
        "taida_str_concat" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_str_length" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_str_char_at" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_str_slice" => RuntimeAbi {
            params: &[Ptr, Val, Val],
            returns: &[Ptr],
        },
        "taida_str_index_of" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Val],
        },
        "taida_str_to_upper" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_str_to_lower" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_str_trim" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_str_split" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_str_replace" => RuntimeAbi {
            params: &[Ptr, Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_str_to_int" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_str_repeat" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_str_reverse" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_str_trim_start" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_str_trim_end" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_str_replace_first" => RuntimeAbi {
            params: &[Ptr, Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_str_pad" => RuntimeAbi {
            params: &[Ptr, Val, Ptr, Val],
            returns: &[Ptr],
        },
        "taida_str_from_int" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_str_from_float" => RuntimeAbi {
            params: &[F64],
            returns: &[Ptr],
        },
        "taida_str_from_bool" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        // Str state check methods
        "taida_str_contains" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Val],
        },
        "taida_str_starts_with" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Val],
        },
        "taida_str_ends_with" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Val],
        },
        "taida_str_last_index_of" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Val],
        },
        "taida_str_get" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        // Str refcount
        "taida_str_retain" => RuntimeAbi {
            params: &[Ptr],
            returns: &[],
        },

        // ── Float 操作 ──
        "taida_float_floor" => RuntimeAbi {
            params: &[F64],
            returns: &[F64],
        },
        "taida_float_ceil" => RuntimeAbi {
            params: &[F64],
            returns: &[F64],
        },
        "taida_float_round" => RuntimeAbi {
            params: &[F64],
            returns: &[F64],
        },
        "taida_float_abs" => RuntimeAbi {
            params: &[F64],
            returns: &[F64],
        },
        "taida_float_to_int" => RuntimeAbi {
            params: &[F64],
            returns: &[Val],
        },
        "taida_float_to_str" => RuntimeAbi {
            params: &[F64],
            returns: &[Ptr],
        },
        "taida_float_to_fixed" => RuntimeAbi {
            params: &[F64, Val],
            returns: &[Ptr],
        },
        "taida_float_clamp" => RuntimeAbi {
            params: &[F64, F64, F64],
            returns: &[F64],
        },
        // Num state check methods (Int)
        "taida_int_is_positive" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_int_is_negative" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_int_is_zero" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        // Num state check methods (Float)
        "taida_float_is_nan" => RuntimeAbi {
            params: &[F64],
            returns: &[Val],
        },
        "taida_float_is_infinite" => RuntimeAbi {
            params: &[F64],
            returns: &[Val],
        },
        "taida_float_is_finite_check" => RuntimeAbi {
            params: &[F64],
            returns: &[Val],
        },
        "taida_float_is_positive" => RuntimeAbi {
            params: &[F64],
            returns: &[Val],
        },
        "taida_float_is_negative" => RuntimeAbi {
            params: &[F64],
            returns: &[Val],
        },
        "taida_float_is_zero" => RuntimeAbi {
            params: &[F64],
            returns: &[Val],
        },

        // ── Bool 変換 ──
        "taida_bool_to_str" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_bool_to_int" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },

        // ── 追加 List 操作 ──
        "taida_list_index_of" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Val],
        },
        "taida_list_last_index_of" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Val],
        },
        "taida_list_any" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Val],
        },
        "taida_list_all" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Val],
        },
        "taida_list_none" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Val],
        },
        "taida_list_concat" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_list_join" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_list_sort" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_list_unique" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_list_flatten" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_list_max" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_list_min" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        // List mold operations
        "taida_list_append" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_list_prepend" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_list_sort_desc" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_list_sort_by" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Ptr],
        },
        "taida_list_find" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Ptr],
        },
        "taida_list_find_index" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Ptr],
        },
        "taida_list_count" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Val],
        },
        "taida_list_fold" => RuntimeAbi {
            params: &[Ptr, Val, FnPtr],
            returns: &[Val],
        },
        "taida_list_foldr" => RuntimeAbi {
            params: &[Ptr, Val, FnPtr],
            returns: &[Val],
        },
        "taida_list_take" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_list_take_while" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Ptr],
        },
        "taida_list_drop" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_list_drop_while" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Ptr],
        },
        "taida_list_zip" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_list_enumerate" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },

        // ── HashMap 操作 ──
        "taida_hashmap_new" => RuntimeAbi {
            params: &[],
            returns: &[Ptr],
        },
        "taida_hashmap_set_value_tag" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[],
        },
        "taida_hashmap_set" => RuntimeAbi {
            params: &[Ptr, Val, Ptr, Val],
            returns: &[Ptr],
        },
        "taida_hashmap_set_immut" => RuntimeAbi {
            params: &[Ptr, Val, Ptr, Val],
            returns: &[Ptr],
        },
        "taida_hashmap_get" => RuntimeAbi {
            params: &[Ptr, Val, Ptr],
            returns: &[Val],
        },
        "taida_hashmap_get_lax" => RuntimeAbi {
            params: &[Ptr, Val, Ptr],
            returns: &[Ptr],
        },
        "taida_hashmap_has" => RuntimeAbi {
            params: &[Ptr, Val, Ptr],
            returns: &[Val],
        },
        "taida_hashmap_remove" => RuntimeAbi {
            params: &[Ptr, Val, Ptr],
            returns: &[Ptr],
        },
        "taida_hashmap_remove_immut" => RuntimeAbi {
            params: &[Ptr, Val, Ptr],
            returns: &[Ptr],
        },
        "taida_hashmap_keys" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_hashmap_values" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_hashmap_length" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_hashmap_is_empty" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_hashmap_entries" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_hashmap_merge" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_hashmap_to_string" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_str_hash" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_value_hash" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },

        // ── Set 操作 ──
        "taida_set_new" => RuntimeAbi {
            params: &[],
            returns: &[Ptr],
        },
        "taida_set_set_elem_tag" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[],
        },
        "taida_set_from_list" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_set_add" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_set_remove" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_set_has" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Val],
        },
        "taida_set_size" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_set_is_empty" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_set_to_list" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_set_union" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_set_intersect" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_set_diff" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_set_to_string" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },

        // ── Polymorphic collection methods ──
        // These dispatch dynamically based on runtime type tag, so args are boxed Val.
        "taida_collection_get" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },
        "taida_collection_has" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },
        "taida_collection_remove" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },
        "taida_collection_size" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_polymorphic_length" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },

        // ── Error ceiling ──
        "taida_error_ceiling_push" => RuntimeAbi {
            params: &[],
            returns: &[Ptr],
        },
        "taida_error_ceiling_pop" => RuntimeAbi {
            params: &[],
            returns: &[],
        },
        "taida_throw" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_error_setjmp" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_error_try_call" => RuntimeAbi {
            params: &[Ptr, FnPtr, Val],
            returns: &[Ptr],
        },
        "taida_error_get_value" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_error_try_get_result" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        // RCB-101: Error type filtering for |==
        "taida_register_type_parent" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[],
        },
        "taida_error_type_matches" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Val],
        },
        "taida_error_type_check_or_rethrow" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Val],
        },

        // ── Result[T, P] ──
        "taida_result_create" => RuntimeAbi {
            params: &[Val, Ptr, FnPtr],
            returns: &[Ptr],
        },
        "taida_result_is_ok" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_result_is_error" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_result_get_or_default" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Val],
        },
        "taida_result_get_or_throw" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_result_map" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Ptr],
        },
        "taida_result_flat_map" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Ptr],
        },
        "taida_result_map_error" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Ptr],
        },
        "taida_result_to_string" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },

        // ── Lax methods ──
        "taida_lax_map" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Ptr],
        },
        "taida_lax_flat_map" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Ptr],
        },
        "taida_lax_to_string" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },

        // ── Polymorphic monadic dispatch ──
        // These dispatch dynamically, so args are boxed Val.
        "taida_polymorphic_map" => RuntimeAbi {
            params: &[Val, FnPtr],
            returns: &[Val],
        },
        "taida_polymorphic_contains" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },
        "taida_polymorphic_index_of" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },
        "taida_polymorphic_last_index_of" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },
        "taida_polymorphic_get_or_default" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },
        "taida_polymorphic_has_value" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_polymorphic_is_empty" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_polymorphic_to_string" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_monadic_flat_map" => RuntimeAbi {
            params: &[Val, FnPtr],
            returns: &[Val],
        },
        "taida_monadic_get_or_throw" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },

        // ── Async methods ──
        "taida_async_map" => RuntimeAbi {
            params: &[Ptr, FnPtr],
            returns: &[Ptr],
        },
        "taida_async_get_or_default" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Val],
        },

        // ── 参照カウント ──
        "taida_retain" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_release" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },

        // ── Async ──
        "taida_async_ok" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_async_ok_tagged" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Ptr],
        },
        "taida_async_err" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_async_set_value_tag" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[],
        },
        "taida_async_unmold" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_async_is_pending" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_async_is_fulfilled" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_async_is_rejected" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_async_get_value" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_async_get_error" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_async_all" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_async_race" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_async_spawn" => RuntimeAbi {
            params: &[FnPtr, Val],
            returns: &[Ptr],
        },
        "taida_async_cancel" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },

        // ── Lax ──
        "taida_lax_new" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Ptr],
        },
        "taida_lax_empty" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_lax_has_value" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_lax_get_or_default" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Val],
        },
        "taida_lax_unmold" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_lax_is_empty" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_generic_unmold" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },

        // ── Gorillax / RelaxedGorillax ──
        "taida_gorillax_new" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_molten_new" => RuntimeAbi {
            params: &[],
            returns: &[Ptr],
        },
        "taida_stub_new" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_todo_new" => RuntimeAbi {
            params: &[Ptr, Ptr, Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_cage_apply" => RuntimeAbi {
            params: &[Val, FnPtr],
            returns: &[Ptr],
        },
        "taida_gorillax_unmold" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_gorillax_relax" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_gorillax_to_string" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_relaxed_gorillax_unmold" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_relaxed_gorillax_to_string" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },

        // ── 型変換モールド (Str/Int/Float/Bool) — 全て Lax (Ptr) を返す ──
        "taida_str_mold_int" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_str_mold_float" => RuntimeAbi {
            params: &[F64],
            returns: &[Ptr],
        },
        "taida_str_mold_bool" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_str_mold_str" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_int_mold_int" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_int_mold_float" => RuntimeAbi {
            params: &[F64],
            returns: &[Ptr],
        },
        "taida_int_mold_str" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_int_mold_auto" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_int_mold_str_base" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_int_mold_bool" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_float_mold_int" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_float_mold_float" => RuntimeAbi {
            params: &[F64],
            returns: &[Ptr],
        },
        "taida_float_mold_str" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_float_mold_bool" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_bool_mold_int" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_bool_mold_float" => RuntimeAbi {
            params: &[F64],
            returns: &[Ptr],
        },
        "taida_bool_mold_str" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_bool_mold_bool" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_uint8_mold" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_uint8_mold_float" => RuntimeAbi {
            params: &[F64],
            returns: &[Ptr],
        },
        "taida_u16be_mold" | "taida_u16le_mold" | "taida_u32be_mold" | "taida_u32le_mold" => {
            RuntimeAbi {
                params: &[Val],
                returns: &[Ptr],
            }
        }
        "taida_u16be_decode_mold"
        | "taida_u16le_decode_mold"
        | "taida_u32be_decode_mold"
        | "taida_u32le_decode_mold" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_bytes_mold" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Ptr],
        },
        "taida_bytes_set" => RuntimeAbi {
            params: &[Ptr, Val, Val],
            returns: &[Ptr],
        },
        "taida_bytes_to_list" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_bytes_cursor_new" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_bytes_cursor_remaining" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_bytes_cursor_take" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_bytes_cursor_u8" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_char_mold_int" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_char_mold_str" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_codepoint_mold_str" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_utf8_encode_mold" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_utf8_decode_mold" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_slice_mold" => RuntimeAbi {
            params: &[Ptr, Val, Val],
            returns: &[Ptr],
        },

        // ── JSON — Molten Iron ──
        "taida_json_schema_cast" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_json_parse" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_json_from_int" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_json_from_str" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_json_unmold" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_json_stringify" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_json_to_str" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_json_to_int" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_json_size" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_json_has" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Val],
        },
        "taida_json_empty" => RuntimeAbi {
            params: &[],
            returns: &[Ptr],
        },

        // ── stdlib math (boxed float as I64 via bitcast) ──
        // These take/return boxed float (I64), not raw F64, because they
        // receive bitcasted values at the user-function level.
        "taida_math_sqrt"
        | "taida_math_abs"
        | "taida_math_sin"
        | "taida_math_cos"
        | "taida_math_tan"
        | "taida_math_asin"
        | "taida_math_acos"
        | "taida_math_atan"
        | "taida_math_log"
        | "taida_math_log10"
        | "taida_math_exp"
        | "taida_math_floor"
        | "taida_math_ceil"
        | "taida_math_round"
        | "taida_math_truncate" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_math_pow" | "taida_math_atan2" | "taida_math_max" | "taida_math_min" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },
        "taida_math_clamp" => RuntimeAbi {
            params: &[Val, Val, Val],
            returns: &[Val],
        },
        // Float arithmetic (boxed float as I64 via bitcast)
        "taida_float_add" | "taida_float_sub" | "taida_float_mul" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Val],
        },

        // ── stdlib I/O ──
        "taida_io_stdout" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_io_stderr" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        "taida_io_stdin" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_sha256" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        // typeof prelude
        "taida_typeof" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Ptr],
        },
        // time prelude
        "taida_time_now_ms" => RuntimeAbi {
            params: &[],
            returns: &[Val],
        },
        "taida_time_sleep" => RuntimeAbi {
            params: &[Val],
            returns: &[Val],
        },
        // jsonEncode / jsonPretty
        "taida_json_encode" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_json_pretty" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        // Field name registry
        "taida_register_field_name" => RuntimeAbi {
            params: &[Val, Ptr],
            returns: &[Val],
        },
        "taida_register_field_type" => RuntimeAbi {
            params: &[Val, Ptr, Val],
            returns: &[Val],
        },

        // ── taida-lang/os package — input molds ──
        "taida_os_read" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_os_read_bytes" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_os_list_dir" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_os_stat" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_os_exists" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        "taida_os_env_var" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        // taida-lang/os package — side-effect functions
        "taida_os_write_file" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_os_write_bytes" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_os_append_file" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_os_remove" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_os_create_dir" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_os_rename" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_os_run" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_os_exec_shell" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        // taida-lang/os package — query function
        "taida_os_all_env" => RuntimeAbi {
            params: &[],
            returns: &[Ptr],
        },
        "taida_os_argv" => RuntimeAbi {
            params: &[],
            returns: &[Ptr],
        },
        // taida-lang/os package — Phase 2: async APIs
        "taida_os_read_async" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_os_http_get" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_os_http_post" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_os_http_request" => RuntimeAbi {
            params: &[Ptr, Ptr, Ptr, Ptr],
            returns: &[Ptr],
        },
        "taida_os_dns_resolve" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_os_tcp_connect" => RuntimeAbi {
            params: &[Ptr, Val, Val],
            returns: &[Ptr],
        },
        "taida_os_tcp_listen" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Ptr],
        },
        "taida_os_tcp_accept" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Ptr],
        },
        "taida_os_socket_send" => RuntimeAbi {
            params: &[Val, Ptr, Val],
            returns: &[Ptr],
        },
        "taida_os_socket_send_all" => RuntimeAbi {
            params: &[Val, Ptr, Val],
            returns: &[Ptr],
        },
        "taida_os_socket_recv" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Ptr],
        },
        "taida_os_socket_send_bytes" => RuntimeAbi {
            params: &[Val, Ptr, Val],
            returns: &[Ptr],
        },
        "taida_os_socket_recv_bytes" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Ptr],
        },
        "taida_os_socket_recv_exact" => RuntimeAbi {
            params: &[Val, Val, Val],
            returns: &[Ptr],
        },
        "taida_os_udp_bind" => RuntimeAbi {
            params: &[Ptr, Val, Val],
            returns: &[Ptr],
        },
        "taida_os_udp_send_to" => RuntimeAbi {
            params: &[Val, Ptr, Val, Ptr, Val],
            returns: &[Ptr],
        },
        "taida_os_udp_recv_from" => RuntimeAbi {
            params: &[Val, Val],
            returns: &[Ptr],
        },
        "taida_os_socket_close" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },
        "taida_os_listener_close" => RuntimeAbi {
            params: &[Val],
            returns: &[Ptr],
        },

        // ── taida-lang/pool package ──
        "taida_pool_create" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_pool_acquire" => RuntimeAbi {
            params: &[Ptr, Val],
            returns: &[Ptr],
        },
        "taida_pool_release" => RuntimeAbi {
            params: &[Ptr, Val, Val],
            returns: &[Ptr],
        },
        "taida_pool_close" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        "taida_pool_health" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },

        // ── taida-lang/net HTTP v1 ──
        // taida_net_http_parse_request_head(input: Ptr) -> Ptr
        "taida_net_http_parse_request_head" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        // taida_net_http_encode_response(response: Ptr) -> Ptr
        "taida_net_http_encode_response" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        // NET3-5a: taida_net_http_serve(port, handler, max_requests, timeout_ms, max_connections, handler_type_tag, handler_arity) -> Ptr
        "taida_net_http_serve" => RuntimeAbi {
            params: &[Val, Ptr, Val, Val, Val, Val, Val],
            returns: &[Ptr],
        },
        // NET2-0f: taida_net_read_body(req: Ptr) -> Ptr (Bytes)
        "taida_net_read_body" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Ptr],
        },
        // NET3-5b: startResponse(writer, status, headers) -> Val
        "taida_net_start_response" => RuntimeAbi {
            params: &[Ptr, Val, Ptr],
            returns: &[Val],
        },
        // NET3-5b: writeChunk(writer, data) -> Val
        "taida_net_write_chunk" => RuntimeAbi {
            params: &[Ptr, Ptr],
            returns: &[Val],
        },
        // NET3-5b: endResponse(writer) -> Val
        "taida_net_end_response" => RuntimeAbi {
            params: &[Ptr],
            returns: &[Val],
        },
        // NET3-5e: sseEvent(writer, event, data) -> Val
        "taida_net_sse_event" => RuntimeAbi {
            params: &[Ptr, Ptr, Ptr],
            returns: &[Val],
        },

        // N-44: ABI table maintenance note
        // When adding a new runtime function in lower.rs, a corresponding entry
        // MUST be added here. The match is exhaustive by design — an unknown
        // function name returns a user-friendly error instead of panicking.
        // To add a new entry:
        //   1. Identify the C signature in native_runtime.c
        //   2. Map each parameter to AbiKind: Val (i64), Ptr (heap ptr), FnPtr, F64
        //   3. Add a `"taida_<name>" => RuntimeAbi { params: &[...], returns: &[...] }` arm above
        _ => {
            return Err(format!(
                "unknown runtime function: '{}'. Add it to the ABI table in emit.rs runtime_abi().",
                name
            ));
        }
    })
}

/// ABI 定義 + AbiHelper から CLIF 型を解決する
///
/// W-0 では `CompileTarget::Native` のみが有効であり、全 AbiKind が I64 に解決
/// されるため、旧 `runtime_func_signature()` と同一の結果を返す。
fn resolve_abi(abi: &RuntimeAbi, helper: &AbiHelper) -> (Vec<clif::Type>, Vec<clif::Type>) {
    let resolve_kind = |k: &AbiKind| -> clif::Type {
        match k {
            AbiKind::Val => helper.value_ty(),
            AbiKind::Ptr => helper.ptr_ty(),
            AbiKind::FnPtr => helper.fn_ptr_ty(),
            AbiKind::F64 => types::F64,
        }
    };
    let params = abi.params.iter().map(resolve_kind).collect();
    let returns = abi.returns.iter().map(resolve_kind).collect();
    (params, returns)
}

/// runtime 関数シグネチャを ABI テーブル経由で解決する
///
/// W-0f: CompileTarget::Native 固定だったラッパーを廃止し、
/// AbiHelper を受け取る形に変更。呼び出し元は self.abi を渡す。
fn runtime_func_signature_for(
    name: &str,
    helper: &AbiHelper,
) -> Result<(Vec<clif::Type>, Vec<clif::Type>), String> {
    let abi = runtime_abi(name)?;
    Ok(resolve_abi(&abi, helper))
}

pub struct Emitter {
    pub module: ObjectModule,
    builder_ctx: FunctionBuilderContext,
    ctx: cranelift_codegen::Context,
    declared_funcs: HashMap<String, FuncId>,
    string_constants: Vec<(cranelift_module::DataId, Vec<u8>)>,
    user_func_sigs: HashMap<String, (Vec<clif::Type>, Vec<clif::Type>)>,
    abi: AbiHelper,
}

/// emit_function 内で使うコンテキスト
struct EmitCtx {
    val_map: HashMap<IrVar, clif::Value>,
    named_vars: HashMap<String, clif::Value>,
    func_refs: HashMap<String, clif::FuncRef>,
    str_globals: HashMap<IrVar, clif::GlobalValue>,
    /// TCO: ループ先頭ブロック（末尾再帰用）
    tco_loop_block: Option<clif::Block>,
    /// TCO: パラメータ Variable のリスト（引数の再代入用）
    tco_param_vars: Vec<cranelift_frontend::Variable>,
    /// ターゲットの呼出規約（CallIndirect 等で使用）
    call_conv: CallConv,
    /// Taida の内部 boxed value 型（AbiHelper::value_ty() のキャッシュ）。
    /// ユーザー関数・IR 変数・内部 SSA の境界では常にこの型を使う。
    /// W-0 では I64 固定。Wasm32 でも I64（案 A: 統一値表現）。
    value_ty: clif::Type,
    /// W-0f: ABI ヘルパー（runtime 関数シグネチャ解決に使用）
    abi: AbiHelper,
}

#[derive(Debug)]
pub struct EmitError {
    pub message: String,
}

impl std::fmt::Display for EmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Emit error: {}", self.message)
    }
}

impl Emitter {
    /// 既存互換: ホストネイティブターゲットで Emitter を生成
    pub fn new() -> Result<Self, EmitError> {
        Self::new_with_target(CompileTarget::Native)
    }

    /// 指定ターゲットで Emitter を生成
    pub fn new_with_target(target: CompileTarget) -> Result<Self, EmitError> {
        let abi = AbiHelper::new(target);

        let shared_builder = settings::builder();
        let shared_flags = settings::Flags::new(shared_builder);

        let triple = abi.triple();
        let isa = cranelift_codegen::isa::lookup(triple.clone())
            .map_err(|e| EmitError {
                message: format!("ISA lookup failed: {}", e),
            })?
            .finish(shared_flags)
            .map_err(|e| EmitError {
                message: format!("ISA finish failed: {}", e),
            })?;

        let obj_builder = ObjectBuilder::new(
            isa,
            "taida_module",
            cranelift_module::default_libcall_names(),
        )
        .map_err(|e| EmitError {
            message: format!("ObjectBuilder failed: {}", e),
        })?;

        let module = ObjectModule::new(obj_builder);

        Ok(Self {
            module,
            builder_ctx: FunctionBuilderContext::new(),
            ctx: cranelift_codegen::Context::new(),
            declared_funcs: HashMap::new(),
            string_constants: Vec::new(),
            user_func_sigs: HashMap::new(),
            abi,
        })
    }

    fn declare_runtime_func(
        &mut self,
        name: &str,
        params: &[clif::Type],
        returns: &[clif::Type],
    ) -> Result<FuncId, EmitError> {
        if let Some(&id) = self.declared_funcs.get(name) {
            return Ok(id);
        }

        let mut sig = self.module.make_signature();
        for &p in params {
            sig.params.push(AbiParam::new(p));
        }
        for &r in returns {
            sig.returns.push(AbiParam::new(r));
        }
        sig.call_conv = self.abi.call_conv();

        let id = self
            .module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|e| EmitError {
                message: format!("declare_function failed: {}", e),
            })?;

        self.declared_funcs.insert(name.to_string(), id);
        Ok(id)
    }

    fn declare_user_func(&mut self, name: &str, param_count: usize) -> Result<FuncId, EmitError> {
        if let Some(&id) = self.declared_funcs.get(name) {
            return Ok(id);
        }

        let vty = self.abi.value_ty();
        let mut sig = self.module.make_signature();
        for _ in 0..param_count {
            sig.params.push(AbiParam::new(vty));
        }
        sig.returns.push(AbiParam::new(vty));
        sig.call_conv = self.abi.call_conv();

        let params: Vec<clif::Type> = (0..param_count).map(|_| vty).collect();
        self.user_func_sigs
            .insert(name.to_string(), (params, vec![vty]));

        let id = self
            .module
            .declare_function(name, Linkage::Local, &sig)
            .map_err(|e| EmitError {
                message: format!("declare_function failed: {}", e),
            })?;

        self.declared_funcs.insert(name.to_string(), id);
        Ok(id)
    }

    fn declare_imported_func(
        &mut self,
        name: &str,
        param_count: usize,
    ) -> Result<FuncId, EmitError> {
        if let Some(&id) = self.declared_funcs.get(name) {
            return Ok(id);
        }
        let vty = self.abi.value_ty();
        let mut sig = self.module.make_signature();
        for _ in 0..param_count {
            sig.params.push(AbiParam::new(vty));
        }
        sig.returns.push(AbiParam::new(vty));
        sig.call_conv = self.abi.call_conv();

        let params: Vec<clif::Type> = (0..param_count).map(|_| vty).collect();
        self.user_func_sigs
            .insert(name.to_string(), (params, vec![vty]));

        let id = self
            .module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|e| EmitError {
                message: format!("declare_imported_func failed: {}", e),
            })?;
        self.declared_funcs.insert(name.to_string(), id);
        Ok(id)
    }

    fn declare_exported_func(
        &mut self,
        name: &str,
        param_count: usize,
    ) -> Result<FuncId, EmitError> {
        if let Some(&id) = self.declared_funcs.get(name) {
            return Ok(id);
        }
        let vty = self.abi.value_ty();
        let mut sig = self.module.make_signature();
        for _ in 0..param_count {
            sig.params.push(AbiParam::new(vty));
        }
        sig.returns.push(AbiParam::new(vty));
        sig.call_conv = self.abi.call_conv();

        let params: Vec<clif::Type> = (0..param_count).map(|_| vty).collect();
        self.user_func_sigs
            .insert(name.to_string(), (params, vec![vty]));

        let id = self
            .module
            .declare_function(name, Linkage::Export, &sig)
            .map_err(|e| EmitError {
                message: format!("declare_exported_func failed: {}", e),
            })?;
        self.declared_funcs.insert(name.to_string(), id);
        Ok(id)
    }

    /// 全 IR 関数からランタイム関数呼び出しを収集して宣言
    fn predeclare_all_runtime_funcs(&mut self, ir_module: &IrModule) -> Result<(), EmitError> {
        for ir_func in &ir_module.functions {
            self.predeclare_runtime_funcs_recursive(&ir_func.body)?;
        }
        Ok(())
    }

    fn predeclare_runtime_funcs_recursive(&mut self, insts: &[IrInst]) -> Result<(), EmitError> {
        for inst in insts {
            match inst {
                IrInst::Call(_, func_name, _) => {
                    if !self.declared_funcs.contains_key(func_name) {
                        let (params, returns) = runtime_func_signature_for(func_name, &self.abi)
                            .map_err(|e| EmitError { message: e })?;
                        self.declare_runtime_func(func_name, &params, &returns)?;
                    }
                }
                IrInst::PackNew(_, _) => {
                    self.ensure_runtime_func("taida_pack_new")?;
                }
                IrInst::PackSet(_, _, _) => {
                    self.ensure_runtime_func("taida_pack_set")?;
                }
                IrInst::PackSetTag(_, _, _) => {
                    self.ensure_runtime_func("taida_pack_set_tag")?;
                }
                IrInst::PackGet(_, _, _) => {
                    self.ensure_runtime_func("taida_pack_get_idx")?;
                }
                IrInst::MakeClosure(_, _, _) => {
                    self.ensure_runtime_func("taida_pack_new")?;
                    self.ensure_runtime_func("taida_pack_set")?;
                    self.ensure_runtime_func("taida_pack_set_hash")?;
                    self.ensure_runtime_func("taida_closure_new")?;
                }
                IrInst::CallIndirect(_, _, _) => {
                    self.ensure_runtime_func("taida_is_closure_value")?;
                    self.ensure_runtime_func("taida_closure_get_fn")?;
                    self.ensure_runtime_func("taida_closure_get_env")?;
                }
                IrInst::Retain(_) => {
                    self.ensure_runtime_func("taida_retain")?;
                }
                IrInst::Release(_) => {
                    self.ensure_runtime_func("taida_release")?;
                }
                IrInst::GlobalSet(_, _) => {
                    self.ensure_runtime_func("taida_global_set")?;
                }
                IrInst::GlobalGet(_, _) => {
                    self.ensure_runtime_func("taida_global_get")?;
                }
                IrInst::CondBranch(_, arms) => {
                    for arm in arms {
                        self.predeclare_runtime_funcs_recursive(&arm.body)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// IR 命令列から CallUser の引数数を収集
    /// IR 命令列から _taida_init_* 関数呼び出しを収集する
    fn collect_init_calls(insts: &[IrInst]) -> Vec<String> {
        let mut result = Vec::new();
        for inst in insts {
            match inst {
                IrInst::CallUser(_, name, _) if name.starts_with("_taida_init_") => {
                    result.push(name.clone());
                }
                IrInst::CondBranch(_, arms) => {
                    for arm in arms {
                        result.extend(Self::collect_init_calls(&arm.body));
                    }
                }
                _ => {}
            }
        }
        result
    }

    fn collect_call_arities(
        insts: &[IrInst],
        targets: &std::collections::HashSet<String>,
        arities: &mut HashMap<String, usize>,
    ) {
        for inst in insts {
            match inst {
                IrInst::CallUser(_, name, args) => {
                    if targets.contains(name) {
                        arities.insert(name.clone(), args.len());
                    }
                }
                IrInst::CondBranch(_, arms) => {
                    for arm in arms {
                        Self::collect_call_arities(&arm.body, targets, arities);
                    }
                }
                _ => {}
            }
        }
    }

    fn ensure_runtime_func(&mut self, name: &str) -> Result<(), EmitError> {
        if !self.declared_funcs.contains_key(name) {
            let (params, returns) = runtime_func_signature_for(name, &self.abi)
                .map_err(|e| EmitError { message: e })?;
            self.declare_runtime_func(name, &params, &returns)?;
        }
        Ok(())
    }

    pub fn emit_module(&mut self, ir_module: &IrModule) -> Result<(), EmitError> {
        // インポートされた関数名の収集
        let imported_funcs: std::collections::HashSet<String> = ir_module
            .imports
            .iter()
            .flat_map(|(_, syms, _)| syms.iter().cloned())
            .collect();

        // IR から各インポート関数の引数数を推定
        let mut import_arities: HashMap<String, usize> = HashMap::new();
        for ir_func in &ir_module.functions {
            Self::collect_call_arities(&ir_func.body, &imported_funcs, &mut import_arities);
        }

        // インポートされた関数を宣言
        for mangled in &imported_funcs {
            let arity = import_arities.get(mangled).copied().unwrap_or(0);
            self.declare_imported_func(mangled, arity)?;
        }

        // モジュール init 関数の宣言（_taida_init_<module_key> は他モジュールからインポート）
        let init_funcs: std::collections::HashSet<String> = ir_module
            .functions
            .iter()
            .flat_map(|f| Self::collect_init_calls(&f.body))
            .filter(|name| {
                // 自モジュールで定義されている init 関数は除外
                !ir_module.functions.iter().any(|f| f.name == *name)
            })
            .collect();
        for init_name in &init_funcs {
            // init 関数は引数なし・i64 戻り値
            self.declare_imported_func(init_name, 0)?;
        }

        // エクスポートされる関数名のセット
        let exported_funcs: std::collections::HashSet<String> =
            ir_module.exports.iter().cloned().collect();

        // 1st pass: 全関数を宣言
        for ir_func in &ir_module.functions {
            if ir_func.name == "_taida_main" {
                let mut sig = self.module.make_signature();
                sig.returns.push(AbiParam::new(self.abi.value_ty()));
                sig.call_conv = self.abi.call_conv();

                let id = self
                    .module
                    .declare_function(&ir_func.name, Linkage::Export, &sig)
                    .map_err(|e| EmitError {
                        message: format!("{}", e),
                    })?;
                self.declared_funcs.insert(ir_func.name.clone(), id);
            } else if exported_funcs.contains(&ir_func.name)
                || ir_func.name.starts_with("_taida_init_")
            {
                self.declare_exported_func(&ir_func.name, ir_func.params.len())?;
            } else {
                self.declare_user_func(&ir_func.name, ir_func.params.len())?;
            }
        }

        // ランタイム関数を宣言
        self.predeclare_all_runtime_funcs(ir_module)?;

        // 2nd pass: 各関数を定義
        for ir_func in &ir_module.functions {
            self.emit_function(ir_func)?;
        }

        // 文字列定数をデータセクションに書き込み
        for (data_id, bytes) in &self.string_constants {
            let mut data_desc = cranelift_module::DataDescription::new();
            data_desc.define(bytes.clone().into_boxed_slice());
            self.module
                .define_data(*data_id, &data_desc)
                .map_err(|e| EmitError {
                    message: format!("define_data failed: {}", e),
                })?;
        }

        Ok(())
    }

    fn collect_str_constants(
        &mut self,
        insts: &[IrInst],
        func_name: &str,
    ) -> Result<HashMap<IrVar, clif::GlobalValue>, EmitError> {
        let mut str_globals = HashMap::new();
        self.collect_str_constants_recursive(insts, func_name, &mut str_globals)?;
        Ok(str_globals)
    }

    fn collect_str_constants_recursive(
        &mut self,
        insts: &[IrInst],
        func_name: &str,
        str_globals: &mut HashMap<IrVar, clif::GlobalValue>,
    ) -> Result<(), EmitError> {
        for inst in insts {
            match inst {
                IrInst::ConstStr(dst, string) => {
                    let mut bytes = string.as_bytes().to_vec();
                    bytes.push(0);

                    let data_id = self
                        .module
                        .declare_data(
                            &format!("{}__str_{}", func_name, dst),
                            Linkage::Local,
                            false,
                            false,
                        )
                        .map_err(|e| EmitError {
                            message: format!("declare_data failed: {}", e),
                        })?;

                    self.string_constants.push((data_id, bytes));

                    let global = self
                        .module
                        .declare_data_in_func(data_id, &mut self.ctx.func);
                    str_globals.insert(*dst, global);
                }
                IrInst::CondBranch(_, arms) => {
                    for arm in arms {
                        self.collect_str_constants_recursive(&arm.body, func_name, str_globals)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// IR 命令列に TailCall が含まれるかチェック（再帰的）
    fn contains_tail_call(insts: &[IrInst]) -> bool {
        for inst in insts {
            match inst {
                IrInst::TailCall(_) => return true,
                IrInst::CondBranch(_, arms) => {
                    for arm in arms {
                        if Self::contains_tail_call(&arm.body) {
                            return true;
                        }
                    }
                }
                _ => {}
            }
        }
        false
    }

    fn emit_function(&mut self, ir_func: &IrFunction) -> Result<(), EmitError> {
        let func_id = self.declared_funcs[&ir_func.name];

        let vty = self.abi.value_ty();
        let mut sig = self.module.make_signature();
        for _ in &ir_func.params {
            sig.params.push(AbiParam::new(vty));
        }
        sig.returns.push(AbiParam::new(vty));
        sig.call_conv = self.abi.call_conv();

        self.ctx.func.signature = sig;
        self.ctx.func.name = cranelift_codegen::ir::UserFuncName::user(0, func_id.as_u32());

        // FuncRef を先に取得
        let mut func_refs: HashMap<String, clif::FuncRef> = HashMap::new();
        for (name, &fid) in &self.declared_funcs {
            let fref = self.module.declare_func_in_func(fid, &mut self.ctx.func);
            func_refs.insert(name.clone(), fref);
        }

        // 文字列定数の GlobalValue を収集
        let str_globals = self.collect_str_constants(&ir_func.body, &ir_func.name)?;

        let has_tail_call = Self::contains_tail_call(&ir_func.body);

        {
            let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_ctx);
            let entry_block = builder.create_block();
            builder.append_block_params_for_function_params(entry_block);
            builder.switch_to_block(entry_block);

            let mut ectx = EmitCtx {
                val_map: HashMap::new(),
                named_vars: HashMap::new(),
                func_refs,
                str_globals,
                tco_loop_block: None,
                tco_param_vars: Vec::new(),
                call_conv: self.abi.call_conv(),
                value_ty: self.abi.value_ty(),
                abi: self.abi,
            };

            if has_tail_call {
                // TCO: パラメータを Variable に格納し、ループブロックを作成
                let mut param_vars = Vec::new();
                let block_params = builder.block_params(entry_block).to_vec();

                for (i, param_name) in ir_func.params.iter().enumerate() {
                    let var = builder.declare_var(vty);
                    builder.def_var(var, block_params[i]);
                    param_vars.push(var);
                    // IrVar のマッピングも設定
                    ectx.val_map.insert(i as IrVar, block_params[i]);
                    ectx.named_vars.insert(param_name.clone(), block_params[i]);
                }

                // ループブロック（エントリから無条件ジャンプ）
                let loop_block = builder.create_block();
                builder.ins().jump(loop_block, &[]);
                builder.seal_block(entry_block);

                builder.switch_to_block(loop_block);
                // loop_block は seal しない（TailCall から戻ってくるため）

                // ループブロックでは Variable から値を読み出す
                for (i, param_name) in ir_func.params.iter().enumerate() {
                    let val = builder.use_var(param_vars[i]);
                    ectx.val_map.insert(i as IrVar, val);
                    ectx.named_vars.insert(param_name.clone(), val);
                }

                ectx.tco_loop_block = Some(loop_block);
                ectx.tco_param_vars = param_vars;

                Self::emit_instructions(&mut builder, &mut ectx, &ir_func.body)?;

                // ループブロックを seal（全ての predecessor が確定した後）
                builder.seal_block(loop_block);
            } else {
                builder.seal_block(entry_block);

                // パラメータをマッピング
                let block_params = builder.block_params(entry_block).to_vec();
                for (i, param_name) in ir_func.params.iter().enumerate() {
                    let param_val = block_params[i];
                    ectx.val_map.insert(i as IrVar, param_val);
                    ectx.named_vars.insert(param_name.clone(), param_val);
                }

                Self::emit_instructions(&mut builder, &mut ectx, &ir_func.body)?;
            }

            builder.finalize();
        }

        self.module
            .define_function(func_id, &mut self.ctx)
            .map_err(|e| EmitError {
                message: format!("define_function failed: {}", e),
            })?;

        self.module.clear_context(&mut self.ctx);
        Ok(())
    }

    /// 命令列を現在のブロックに emit する
    /// NTH-3: Result 伝播対応 — runtime_abi 解決失敗時に panic ではなく EmitError を返す
    fn emit_instructions(
        builder: &mut FunctionBuilder,
        ectx: &mut EmitCtx,
        insts: &[IrInst],
    ) -> Result<(), EmitError> {
        for inst in insts {
            Self::emit_inst(builder, ectx, inst)?;
        }
        Ok(())
    }

    fn emit_inst(
        builder: &mut FunctionBuilder,
        ectx: &mut EmitCtx,
        inst: &IrInst,
    ) -> Result<(), EmitError> {
        match inst {
            IrInst::ConstInt(dst, value) => {
                let val = builder.ins().iconst(ectx.value_ty, *value);
                ectx.val_map.insert(*dst, val);
            }
            IrInst::ConstFloat(dst, value) => {
                let val = builder.ins().f64const(*value);
                ectx.val_map.insert(*dst, val);
            }
            IrInst::ConstStr(dst, _) => {
                // W-0 note: ConstStr の結果は意味的にはヒープ参照（Ptr）だが、
                // 案 A（統一値表現）により boxed value_ty として保持する。
                // Wasm32 では runtime 境界呼び出し時に value_ty → ptr_ty の truncate が入る。
                let global = ectx.str_globals[dst];
                let ptr = builder.ins().global_value(ectx.value_ty, global);
                ectx.val_map.insert(*dst, ptr);
            }
            IrInst::ConstBool(dst, value) => {
                let val = builder
                    .ins()
                    .iconst(ectx.value_ty, if *value { 1 } else { 0 });
                ectx.val_map.insert(*dst, val);
            }
            IrInst::DefVar(name, src) => {
                if let Some(&val) = ectx.val_map.get(src) {
                    ectx.named_vars.insert(name.clone(), val);
                }
            }
            IrInst::UseVar(dst, name) => {
                if let Some(&val) = ectx.named_vars.get(name) {
                    ectx.val_map.insert(*dst, val);
                } else {
                    let val = builder.ins().iconst(ectx.value_ty, 0);
                    ectx.val_map.insert(*dst, val);
                }
            }
            IrInst::PackNew(dst, field_count) => {
                // taida_pack_new(field_count: Val) -> Ptr
                let func_ref = ectx.func_refs["taida_pack_new"];
                let count_val = builder.ins().iconst(ectx.value_ty, *field_count as i64);
                let call = builder.ins().call(func_ref, &[count_val]);
                let results = builder.inst_results(call);
                ectx.val_map.insert(*dst, results[0]);
            }
            IrInst::PackSet(pack_var, index, value_var) => {
                // taida_pack_set(Ptr, Val, Val) -> Ptr
                let func_ref = ectx.func_refs["taida_pack_set"];
                let pack_val = ectx.val_map[pack_var];
                let idx_val = builder.ins().iconst(ectx.value_ty, *index as i64);
                let value_val = ectx.val_map[value_var];
                builder
                    .ins()
                    .call(func_ref, &[pack_val, idx_val, value_val]);
            }
            IrInst::PackSetTag(pack_var, index, tag) => {
                // taida_pack_set_tag(Ptr, Val, Val) -> Ptr
                let func_ref = ectx.func_refs["taida_pack_set_tag"];
                let pack_val = ectx.val_map[pack_var];
                let idx_val = builder.ins().iconst(ectx.value_ty, *index as i64);
                let tag_val = builder.ins().iconst(ectx.value_ty, *tag);
                builder.ins().call(func_ref, &[pack_val, idx_val, tag_val]);
            }
            IrInst::PackGet(dst, pack_var, index) => {
                // taida_pack_get_idx(Ptr, Val) -> Val
                let func_ref = ectx.func_refs["taida_pack_get_idx"];
                let pack_val = ectx.val_map[pack_var];
                let idx_val = builder.ins().iconst(ectx.value_ty, *index as i64);
                let call = builder.ins().call(func_ref, &[pack_val, idx_val]);
                let results = builder.inst_results(call);
                ectx.val_map.insert(*dst, results[0]);
            }
            IrInst::Call(dst, func_name, args) => {
                // ── Integer/Bool intrinsics: emit native instructions instead of call ──
                if let Some(result) = Self::try_emit_intrinsic(builder, ectx, func_name, args) {
                    ectx.val_map.insert(*dst, result);
                } else {
                    // W-0f: runtime_func_signature_for() で ectx.abi ベースの解決に統一
                    let func_ref = ectx.func_refs[func_name];
                    // FL-11 / NTH-3: runtime_abi already validated in predeclare_runtime_funcs_recursive.
                    // Propagate error via Result instead of panicking.
                    let runtime_abi_def = runtime_abi(func_name).map_err(|e| EmitError {
                        message: format!(
                            "BUG: runtime_abi lookup failed for '{}': {}",
                            func_name, e
                        ),
                    })?;
                    let (param_types, return_types) = resolve_abi(&runtime_abi_def, &ectx.abi);

                    let arg_vals: Vec<clif::Value> = args
                        .iter()
                        .enumerate()
                        .map(|(i, &arg)| {
                            let val = ectx.val_map[&arg];
                            let val_type = builder.func.dfg.value_type(val);
                            let expected_type =
                                param_types.get(i).copied().unwrap_or(ectx.value_ty);

                            if val_type == expected_type {
                                val
                            } else if val_type == types::F64 && expected_type == ectx.value_ty {
                                // F64 → boxed value: bitcast to I64
                                builder
                                    .ins()
                                    .bitcast(ectx.value_ty, clif::MemFlags::new(), val)
                            } else if val_type == ectx.value_ty && expected_type == types::F64 {
                                // boxed value → F64: bitcast
                                builder
                                    .ins()
                                    .bitcast(types::F64, clif::MemFlags::new(), val)
                            } else if val_type == ectx.value_ty
                                && (expected_type == ectx.abi.ptr_ty()
                                    || expected_type == ectx.abi.fn_ptr_ty())
                                && val_type != expected_type
                            {
                                // W-0f F-2: boxed value_ty → ptr_ty/fn_ptr_ty (truncate)
                                // Wasm32: I64 → I32
                                builder.ins().ireduce(expected_type, val)
                            } else if (val_type == ectx.abi.ptr_ty()
                                || val_type == ectx.abi.fn_ptr_ty())
                                && expected_type == ectx.value_ty
                                && val_type != expected_type
                            {
                                // W-0f F-2: ptr_ty/fn_ptr_ty → boxed value_ty (extend)
                                // Wasm32: I32 → I64
                                builder.ins().uextend(expected_type, val)
                            } else {
                                val
                            }
                        })
                        .collect();

                    let call = builder.ins().call(func_ref, &arg_vals);
                    let results = builder.inst_results(call);
                    if !results.is_empty() {
                        let result = results[0];
                        let result_type = builder.func.dfg.value_type(result);
                        // W-0f F-2: runtime 戻り値が ptr_ty/fn_ptr_ty の場合、boxed value_ty に変換
                        let boxed = if result_type != ectx.value_ty
                            && (result_type == ectx.abi.ptr_ty()
                                || result_type == ectx.abi.fn_ptr_ty())
                        {
                            builder.ins().uextend(ectx.value_ty, result)
                        } else {
                            result
                        };
                        ectx.val_map.insert(*dst, boxed);
                    } else if return_types.is_empty() {
                        // void 関数: trap の代わりに 0 を返す
                        // (trap はブロック終端命令なので後続命令を追加できない)
                        let dummy = builder.ins().iconst(ectx.value_ty, 0);
                        ectx.val_map.insert(*dst, dummy);
                    }
                }
            }
            IrInst::CallUser(dst, func_name, args) => {
                let func_ref = ectx.func_refs[func_name];
                let arg_vals: Vec<clif::Value> =
                    args.iter().map(|&arg| ectx.val_map[&arg]).collect();

                let call = builder.ins().call(func_ref, &arg_vals);
                let results = builder.inst_results(call);
                if !results.is_empty() {
                    ectx.val_map.insert(*dst, results[0]);
                }
            }
            IrInst::FuncAddr(dst, func_name) => {
                // W-0 note: FuncAddr は意味的には関数ポインタ（FnPtr）だが、
                // 案 A（統一値表現）により boxed value_ty として保持する。
                let fn_ref = ectx.func_refs[func_name];
                let fn_addr = builder.ins().func_addr(ectx.value_ty, fn_ref);
                ectx.val_map.insert(*dst, fn_addr);
            }
            IrInst::MakeClosure(dst, func_name, captures) => {
                // 1. 環境パックを作成（キャプチャ変数を格納）
                let pack_new_ref = ectx.func_refs["taida_pack_new"];
                let count_val = builder.ins().iconst(ectx.value_ty, captures.len() as i64);
                let pack_call = builder.ins().call(pack_new_ref, &[count_val]);
                let env_ptr = builder.inst_results(pack_call)[0];

                // キャプチャ変数を環境に格納
                let pack_set_ref = ectx.func_refs["taida_pack_set"];
                for (i, cap_name) in captures.iter().enumerate() {
                    if let Some(&cap_val) = ectx.named_vars.get(cap_name) {
                        let idx_val = builder.ins().iconst(ectx.value_ty, i as i64);
                        builder
                            .ins()
                            .call(pack_set_ref, &[env_ptr, idx_val, cap_val]);
                    }
                }

                // 2. 関数アドレスを取得（boxed value_ty として保持）
                let fn_ref = ectx.func_refs[func_name];
                let fn_addr = builder.ins().func_addr(ectx.value_ty, fn_ref);

                // 3. クロージャ構造体を作成
                let closure_new_ref = ectx.func_refs["taida_closure_new"];
                let closure_call = builder.ins().call(closure_new_ref, &[fn_addr, env_ptr]);
                let closure_ptr = builder.inst_results(closure_call)[0];

                ectx.val_map.insert(*dst, closure_ptr);
            }
            IrInst::CallIndirect(dst, fn_var, args) => {
                // CallIndirect: ユーザー関数 ABI（全値 value_ty）
                let callable = ectx.val_map[fn_var];
                let result_var = builder.declare_var(ectx.value_ty);
                let closure_block = builder.create_block();
                let plain_block = builder.create_block();
                let merge_block = builder.create_block();

                let is_closure_ref = ectx.func_refs["taida_is_closure_value"];
                let is_closure_call = builder.ins().call(is_closure_ref, &[callable]);
                let is_closure = builder.inst_results(is_closure_call)[0];
                builder
                    .ins()
                    .brif(is_closure, closure_block, &[], plain_block, &[]);

                builder.switch_to_block(closure_block);
                builder.seal_block(closure_block);
                let get_fn_ref = ectx.func_refs["taida_closure_get_fn"];
                let fn_call = builder.ins().call(get_fn_ref, &[callable]);
                let fn_ptr = builder.inst_results(fn_call)[0];

                let get_env_ref = ectx.func_refs["taida_closure_get_env"];
                let env_call = builder.ins().call(get_env_ref, &[callable]);
                let env_ptr = builder.inst_results(env_call)[0];

                let mut closure_args = vec![env_ptr];
                for &arg in args {
                    closure_args.push(ectx.val_map[&arg]);
                }
                let mut closure_sig = clif::Signature::new(ectx.call_conv);
                for _ in &closure_args {
                    closure_sig.params.push(AbiParam::new(ectx.value_ty));
                }
                closure_sig.returns.push(AbiParam::new(ectx.value_ty));
                let closure_sig_ref = builder.import_signature(closure_sig);
                let closure_call =
                    builder
                        .ins()
                        .call_indirect(closure_sig_ref, fn_ptr, &closure_args);
                let closure_result = builder.inst_results(closure_call)[0];
                builder.def_var(result_var, closure_result);
                builder.ins().jump(merge_block, &[]);

                builder.switch_to_block(plain_block);
                builder.seal_block(plain_block);
                let plain_args: Vec<clif::Value> =
                    args.iter().map(|&arg| ectx.val_map[&arg]).collect();
                let mut plain_sig = clif::Signature::new(ectx.call_conv);
                for _ in &plain_args {
                    plain_sig.params.push(AbiParam::new(ectx.value_ty));
                }
                plain_sig.returns.push(AbiParam::new(ectx.value_ty));
                let plain_sig_ref = builder.import_signature(plain_sig);
                let plain_call = builder
                    .ins()
                    .call_indirect(plain_sig_ref, callable, &plain_args);
                let plain_result = builder.inst_results(plain_call)[0];
                builder.def_var(result_var, plain_result);
                builder.ins().jump(merge_block, &[]);

                builder.switch_to_block(merge_block);
                builder.seal_block(merge_block);
                let result = builder.use_var(result_var);
                ectx.val_map.insert(*dst, result);
            }
            IrInst::Retain(var) => {
                let val = ectx.val_map[var];
                if let Some(&func_ref) = ectx.func_refs.get("taida_retain") {
                    builder.ins().call(func_ref, &[val]);
                }
            }
            IrInst::Release(var) => {
                let val = ectx.val_map[var];
                if let Some(&func_ref) = ectx.func_refs.get("taida_release") {
                    builder.ins().call(func_ref, &[val]);
                }
            }
            IrInst::TailCall(args) => {
                // 末尾再帰: 引数を Variable に再代入してループブロックにジャンプ
                if let Some(loop_block) = ectx.tco_loop_block {
                    // 新しい引数値を全て先に評価してから代入
                    // （引数間に依存がある場合の正しさを保証）
                    let arg_vals: Vec<clif::Value> =
                        args.iter().map(|&arg| ectx.val_map[&arg]).collect();

                    for (i, val) in arg_vals.iter().enumerate() {
                        if i < ectx.tco_param_vars.len() {
                            builder.def_var(ectx.tco_param_vars[i], *val);
                        }
                    }

                    builder.ins().jump(loop_block, &[]);

                    // TailCall の後にコードが続く場合のために新しいブロック
                    let next_block = builder.create_block();
                    builder.switch_to_block(next_block);
                    builder.seal_block(next_block);
                }
            }
            IrInst::CondBranch(dst, arms) => {
                Self::emit_cond_branch(builder, ectx, *dst, arms)?;
            }
            IrInst::Return(var) => {
                let val = ectx.val_map[var];
                builder.ins().return_(&[val]);
                let next_block = builder.create_block();
                builder.switch_to_block(next_block);
                builder.seal_block(next_block);
            }
            IrInst::GlobalSet(name_hash, value_var) => {
                let hash_val = builder.ins().iconst(ectx.value_ty, *name_hash);
                let val = ectx.val_map[value_var];
                let func_ref = ectx.func_refs["taida_global_set"];
                builder.ins().call(func_ref, &[hash_val, val]);
            }
            IrInst::GlobalGet(dst, name_hash) => {
                let hash_val = builder.ins().iconst(ectx.value_ty, *name_hash);
                let func_ref = ectx.func_refs["taida_global_get"];
                let call = builder.ins().call(func_ref, &[hash_val]);
                let result = builder.inst_results(call)[0];
                ectx.val_map.insert(*dst, result);
            }
        }
        Ok(())
    }

    /// 条件分岐を CLIF ブロックに変換
    /// Variable を使って結果を受け渡す（BlockArg を回避）
    fn emit_cond_branch(
        builder: &mut FunctionBuilder,
        ectx: &mut EmitCtx,
        result_dst: IrVar,
        arms: &[CondArm],
    ) -> Result<(), EmitError> {
        // 結果を格納する Variable（boxed value_ty）
        let result_var = builder.declare_var(ectx.value_ty);

        // マージブロック
        let merge_block = builder.create_block();

        // 各アームを処理
        for (i, arm) in arms.iter().enumerate() {
            match &arm.condition {
                Some(cond_var) => {
                    let then_block = builder.create_block();
                    let else_block = if i + 1 < arms.len() {
                        builder.create_block()
                    } else {
                        merge_block
                    };

                    let cond_val = ectx.val_map[cond_var];
                    builder
                        .ins()
                        .brif(cond_val, then_block, &[], else_block, &[]);

                    // then ブロック
                    builder.switch_to_block(then_block);
                    builder.seal_block(then_block);
                    Self::emit_instructions(builder, ectx, &arm.body)?;
                    let result_val = ectx.val_map[&arm.result];
                    builder.def_var(result_var, result_val);
                    builder.ins().jump(merge_block, &[]);

                    // else ブロック（次のアームへ）
                    if else_block != merge_block {
                        builder.switch_to_block(else_block);
                        builder.seal_block(else_block);
                    }
                }
                None => {
                    // デフォルトケース (| _ |>)
                    Self::emit_instructions(builder, ectx, &arm.body)?;
                    let result_val = ectx.val_map[&arm.result];
                    builder.def_var(result_var, result_val);
                    builder.ins().jump(merge_block, &[]);
                }
            }
        }

        // デフォルトケースがない場合のフォールバック
        let has_default = arms.iter().any(|a| a.condition.is_none());
        if !has_default {
            let default_val = builder.ins().iconst(ectx.value_ty, 0);
            builder.def_var(result_var, default_val);
            builder.ins().jump(merge_block, &[]);
        }

        builder.switch_to_block(merge_block);
        builder.seal_block(merge_block);

        let result = builder.use_var(result_var);
        ectx.val_map.insert(result_dst, result);
        Ok(())
    }

    /// Simple integer/bool operations → native Cranelift instructions.
    /// Returns Some(result_value) if the function was inlined, None otherwise.
    fn try_emit_intrinsic(
        builder: &mut FunctionBuilder,
        ectx: &mut EmitCtx,
        func_name: &str,
        args: &[IrVar],
    ) -> Option<clif::Value> {
        use cranelift_codegen::ir::condcodes::IntCC;

        match func_name {
            // ── Binary integer arithmetic ──
            "taida_int_add" if args.len() == 2 => {
                let a = ectx.val_map[&args[0]];
                let b = ectx.val_map[&args[1]];
                Some(builder.ins().iadd(a, b))
            }
            "taida_int_sub" if args.len() == 2 => {
                let a = ectx.val_map[&args[0]];
                let b = ectx.val_map[&args[1]];
                Some(builder.ins().isub(a, b))
            }
            "taida_int_mul" if args.len() == 2 => {
                let a = ectx.val_map[&args[0]];
                let b = ectx.val_map[&args[1]];
                Some(builder.ins().imul(a, b))
            }

            // ── Integer comparisons (return i64: 0 or 1) ──
            "taida_int_eq" if args.len() == 2 => {
                let a = ectx.val_map[&args[0]];
                let b = ectx.val_map[&args[1]];
                let cmp = builder.ins().icmp(IntCC::Equal, a, b);
                Some(builder.ins().uextend(types::I64, cmp))
            }
            "taida_int_neq" if args.len() == 2 => {
                let a = ectx.val_map[&args[0]];
                let b = ectx.val_map[&args[1]];
                let cmp = builder.ins().icmp(IntCC::NotEqual, a, b);
                Some(builder.ins().uextend(types::I64, cmp))
            }
            "taida_int_lt" if args.len() == 2 => {
                let a = ectx.val_map[&args[0]];
                let b = ectx.val_map[&args[1]];
                let cmp = builder.ins().icmp(IntCC::SignedLessThan, a, b);
                Some(builder.ins().uextend(types::I64, cmp))
            }
            "taida_int_gt" if args.len() == 2 => {
                let a = ectx.val_map[&args[0]];
                let b = ectx.val_map[&args[1]];
                let cmp = builder.ins().icmp(IntCC::SignedGreaterThan, a, b);
                Some(builder.ins().uextend(types::I64, cmp))
            }
            "taida_int_gte" if args.len() == 2 => {
                let a = ectx.val_map[&args[0]];
                let b = ectx.val_map[&args[1]];
                let cmp = builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, a, b);
                Some(builder.ins().uextend(types::I64, cmp))
            }

            // ── Unary integer ──
            "taida_int_neg" if args.len() == 1 => {
                let a = ectx.val_map[&args[0]];
                Some(builder.ins().ineg(a))
            }

            // ── Boolean operations ──
            "taida_bool_not" if args.len() == 1 => {
                let a = ectx.val_map[&args[0]];
                let one = builder.ins().iconst(types::I64, 1);
                Some(builder.ins().bxor(a, one))
            }
            "taida_bool_and" if args.len() == 2 => {
                let a = ectx.val_map[&args[0]];
                let b = ectx.val_map[&args[1]];
                Some(builder.ins().band(a, b))
            }
            "taida_bool_or" if args.len() == 2 => {
                let a = ectx.val_map[&args[0]];
                let b = ectx.val_map[&args[1]];
                Some(builder.ins().bor(a, b))
            }

            _ => None,
        }
    }
}
