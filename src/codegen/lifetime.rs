//! Codegen lifetime pass for native reference counting.
//!
//! The lowering emits a function-end `Release` only for a handful of heap
//! binding forms, and that release never runs on a tail-recursive
//! back-edge (`TailCall` jumps to the loop header) or for a value made
//! inside a condition arm. This pass runs after lowering and before
//! `rc_opt` and decides, from the IR itself, which references a function
//! owns and when they can be given up:
//!
//! 1. It removes function-end releases that would free a value that is
//!    still in use: a name bound only inside a condition arm (its value
//!    does not dominate the function end, and the arm may yield it), and a
//!    name whose value can outlive the function, in particular through a
//!    returned closure (closure environments borrow what they capture).
//! 2. It inserts `ReleaseAuto(value)` right after `DefVar(name, value)`
//!    when the binding owns its value and nothing references either again
//!    (and the binding is not what its condition arm yields).
//! 3. Outside `_taida_main`, it releases every other value the function
//!    owns at the point where the value's scope is left: before each
//!    `TailCall` and `Return`, and at the end of the condition arm that
//!    made it. A tail-recursive loop therefore no longer keeps the
//!    previous iteration's values alive.
//! 4. In a self tail-recursive loop it tracks, per parameter, whether the
//!    current value was created by the loop (`SetParamOwned`), and
//!    releases the previous iteration's value once the next one replaces
//!    it (`ReleaseParamIfOwned`). A `HashMap.set` whose result becomes the
//!    parameter's next value updates the map in place when the loop holds
//!    the only reference (`taida_hashmap_set_consume`).
//!
//! `ReleaseAuto` calls `taida_release_any`, which dispatches between the
//! heap-string layout (hidden header) and the magic-header layout (Pack /
//! List / Closure / ...) at run time.
//!
//! ## Ownership
//!
//! Releasing gives up one reference, so it is only correct for a value
//! that holds a reference nobody else is counting on. A value is owned
//! when it was produced by `PackNew` / `MakeClosure`, by a runtime call
//! that always returns a newly allocated object ([`FRESH_RESULT_CALLS`]),
//! by a user function whose every return yields such a value, or by a
//! condition whose every arm does. Everything else is borrowed: an alias
//! of another name (`UseVar`), a field or captured value read back from a
//! pack (`PackGet`, as in closure environments, error-ceiling bodies and
//! type methods), a global, a parameter, and the result of any call that
//! may hand back one of its arguments or a part of one.
//!
//! ## When an owned value is still needed
//!
//! The pass records, for every IR value, which other values refer to it
//! and how ([`Edge`]): as the same object (an alias, a branch result), as
//! a borrowed pointer (a field read, a closure capture, a store without a
//! retain, the result of a call that may return an argument), or with a
//! reference of its own (a retained store, a copy made by a runtime call).
//! An owned value can be released at a point when no value that is still
//! reachable after that point (an operand of the `TailCall` / `Return`
//! being reached, something kept by the runtime or a global, the value
//! the arm yields) refers to it through an alias or a borrowed pointer.
//! A value that is held with a reference of its own keeps its referent
//! alive by itself.

use super::ir::*;
use std::collections::{BTreeSet, HashMap, HashSet};

/// Runtime calls whose result is always a newly allocated object that
/// carries one reference for the caller. None of them hands back an
/// argument or a borrowed part of one, and every heap value they copy in
/// is retained by the new object, so releasing the result never touches a
/// value somebody else owns.
///
/// Deliberately absent: accessors and unmolds (`taida_pack_get`,
/// `taida_lax_unmold`, `taida_polymorphic_get_or_default`, ...), calls
/// that can return an argument unchanged (`taida_set_add_tagged`,
/// `taida_result_map_error`, `taida_list_fold`, ...), and calls that store
/// an argument without retaining it (`taida_list_append_k`,
/// `taida_list_prepend`, `taida_list_map_k` for its callback results).
const FRESH_RESULT_CALLS: &[&str] = &[
    // strings
    "taida_str_concat",
    "taida_str_repeat",
    "taida_str_repeat_join",
    "taida_str_replace",
    "taida_str_replace_first",
    "taida_str_replace_poly",
    "taida_str_replace_first_poly",
    "taida_str_to_lower",
    "taida_str_to_upper",
    "taida_str_trim",
    "taida_str_trim_start",
    "taida_str_trim_end",
    "taida_str_pad",
    "taida_str_reverse",
    "taida_str_slice",
    "taida_str_byte_slice",
    "taida_str_char_at",
    "taida_str_from_bool",
    "taida_str_from_int",
    "taida_float_to_str",
    "taida_float_to_fixed",
    "taida_polymorphic_to_string",
    "taida_function_to_string",
    "taida_list_join",
    "taida_json_encode",
    "taida_json_pretty",
    "taida_json_encode_desc",
    "taida_json_pretty_desc",
    "taida_redact",
    "taida_typeof",
    "taida_type_name",
    // lists, sets and maps (elements copied in are retained)
    "taida_list_new",
    "taida_list_filter",
    "taida_list_reverse",
    "taida_list_concat",
    "taida_list_take",
    "taida_list_drop",
    "taida_list_take_while",
    "taida_list_drop_while",
    "taida_list_sort",
    "taida_list_sort_desc",
    "taida_list_sort_by",
    "taida_list_sort_kind",
    "taida_list_unique",
    "taida_list_unique_by",
    "taida_list_zip",
    "taida_list_enumerate",
    "taida_list_flatten",
    "taida_range",
    "taida_str_split",
    "taida_str_split_poly",
    "taida_str_chars",
    "taida_slice_mold",
    "taida_hashmap_new",
    "taida_hashmap_set_immut",
    "taida_hashmap_merge",
    "taida_hashmap_keys",
    "taida_hashmap_values",
    "taida_hashmap_entries",
    "taida_hashmap_from",
    "taida_set_from_list",
    "taida_set_to_list",
    "taida_set_union",
    "taida_set_intersect",
    "taida_set_diff",
    // Lax / Result wrappers (the wrapped value is retained)
    "taida_lax_new",
    "taida_lax_empty",
    "taida_div_mold",
    "taida_mod_mold",
    "taida_div_mold_f",
    "taida_mod_mold_f",
    "taida_list_get",
    "taida_list_first",
    "taida_list_last",
    "taida_list_max",
    "taida_list_min",
    "taida_list_find",
    "taida_collection_get",
    "taida_str_get",
    "taida_str_byte_at_lax",
    "taida_str_mold_int",
    "taida_str_mold_float",
    "taida_str_mold_bool",
    "taida_str_mold_str",
    "taida_str_mold_any",
    "taida_int_mold_int",
    "taida_int_mold_float",
    "taida_int_mold_str",
    "taida_int_mold_bool",
    "taida_int_mold_auto",
    "taida_float_mold_int",
    "taida_float_mold_float",
    "taida_float_mold_str",
    "taida_float_mold_bool",
    "taida_bool_mold_int",
    "taida_bool_mold_float",
    "taida_bool_mold_str",
    "taida_bool_mold_bool",
    "taida_char_mold_int",
    "taida_char_mold_str",
    "taida_codepoint_mold_str",
    "taida_to_radix",
    "taida_result_create",
];

/// Runtime calls whose result is a plain scalar (Int / Float / Bool bits),
/// so it can never point at, or into, one of the arguments.
const SCALAR_RESULT_CALLS: &[&str] = &[
    "taida_int_add",
    "taida_int_sub",
    "taida_int_mul",
    "taida_int_neg",
    "taida_int_abs",
    "taida_int_eq",
    "taida_int_neq",
    "taida_int_lt",
    "taida_int_gt",
    "taida_int_gte",
    "taida_int_lte",
    "taida_int_to_float",
    "taida_float_add",
    "taida_float_sub",
    "taida_float_mul",
    "taida_float_neg",
    "taida_float_eq",
    "taida_float_neq",
    "taida_float_lt",
    "taida_float_gt",
    "taida_float_gte",
    "taida_float_lte",
    "taida_float_to_int",
    "taida_bool_and",
    "taida_bool_or",
    "taida_bool_not",
    "taida_poly_eq",
    "taida_poly_neq",
    "taida_poly_eq_tagged",
    "taida_poly_neq_tagged",
    "taida_str_eq",
    "taida_str_neq",
    "taida_str_lt",
    "taida_str_gt",
    "taida_str_gte",
    "taida_str_starts_with",
    "taida_str_ends_with",
    "taida_str_byte_length",
    "taida_str_byte_at",
    "taida_polymorphic_length",
    "taida_polymorphic_contains",
    "taida_polymorphic_is_empty",
    "taida_polymorphic_has_value",
    "taida_polymorphic_index_of",
    "taida_polymorphic_last_index_of",
    "taida_collection_size",
    "taida_collection_has_tagged",
    "taida_value_hash",
    "taida_io_stdout",
    "taida_io_stdout_with_tag",
    "taida_io_stderr",
    "taida_io_stderr_with_tag",
    "taida_get_call_arg_tag",
    "taida_get_return_tag",
    "taida_set_call_arg_tag",
    "taida_set_return_tag",
    "taida_push_call_tags",
    "taida_pop_call_tags",
    "taida_list_note_push_ekind",
    "taida_list_note_push_dyn",
    "taida_list_set_elem_tag",
    "taida_hashmap_set_value_tag",
    "taida_hashmap_note_kinds",
    "taida_register_enum_variants",
    "taida_register_field_name",
    "taida_register_field_type",
    "taida_register_field_enum",
    "taida_str_retain",
    "taida_list_count",
    "taida_list_any",
    "taida_list_all",
    "taida_list_none",
    "taida_list_find_index",
    "taida_list_sum",
];

/// Fresh-result calls whose result holds no reference to any argument or
/// to anything inside one: they build strings, parse into scalars, or wrap
/// scalars. (The rest of [`FRESH_RESULT_CALLS`] retain what they copy in.)
const COPY_RESULT_CALLS: &[&str] = &[
    "taida_str_concat",
    "taida_str_repeat",
    "taida_str_repeat_join",
    "taida_str_replace",
    "taida_str_replace_first",
    "taida_str_replace_poly",
    "taida_str_replace_first_poly",
    "taida_str_to_lower",
    "taida_str_to_upper",
    "taida_str_trim",
    "taida_str_trim_start",
    "taida_str_trim_end",
    "taida_str_pad",
    "taida_str_reverse",
    "taida_str_slice",
    "taida_str_byte_slice",
    "taida_str_char_at",
    "taida_str_from_bool",
    "taida_str_from_int",
    "taida_float_to_str",
    "taida_float_to_fixed",
    "taida_polymorphic_to_string",
    "taida_function_to_string",
    "taida_list_join",
    "taida_json_encode",
    "taida_json_pretty",
    "taida_json_encode_desc",
    "taida_json_pretty_desc",
    "taida_redact",
    "taida_typeof",
    "taida_type_name",
    "taida_str_split",
    "taida_str_split_poly",
    "taida_str_chars",
    "taida_range",
    "taida_div_mold",
    "taida_mod_mold",
    "taida_div_mold_f",
    "taida_mod_mold_f",
    "taida_str_get",
    "taida_str_byte_at_lax",
    "taida_str_mold_int",
    "taida_str_mold_float",
    "taida_str_mold_bool",
    "taida_int_mold_int",
    "taida_int_mold_float",
    "taida_int_mold_str",
    "taida_int_mold_bool",
    "taida_int_mold_auto",
    "taida_float_mold_int",
    "taida_float_mold_float",
    "taida_float_mold_str",
    "taida_float_mold_bool",
    "taida_bool_mold_int",
    "taida_bool_mold_float",
    "taida_bool_mold_str",
    "taida_bool_mold_bool",
    "taida_char_mold_int",
    "taida_char_mold_str",
    "taida_codepoint_mold_str",
    "taida_to_radix",
];

/// Runtime calls that only read their arguments and return one of them or
/// a value held inside one (no reference of its own).
const ACCESSOR_CALLS: &[&str] = &[
    "taida_pack_get",
    "taida_error_field",
    "taida_register_fn_name",
    "taida_pack_get_idx",
    "taida_pack_set_hash",
    "taida_pack_set_tag",
    "taida_lax_unmold",
    "taida_generic_unmold",
    "taida_gorillax_unmold",
    "taida_relaxed_gorillax_unmold",
    "taida_lax_get_or_default",
    "taida_polymorphic_get_or_default",
    "taida_monadic_get_or_throw",
    "taida_lax_tag_value_default",
    "taida_lax_fill_default",
    "taida_result_throw_field",
];

/// Runtime calls that only read their arguments while they run (they may
/// call a function argument, but keep nothing afterwards). Their result may
/// still point into an argument without a reference of its own. Calls that
/// appear in none of these lists are assumed to keep their arguments (async
/// tasks, registries, the error slot, I/O handles, ...), so nothing passed
/// to them is ever released by this pass.
const READ_ONLY_ARG_CALLS: &[&str] = &[
    "taida_pack_get_field_tag",
    "taida_pack_has_hash",
    "taida_lax_value_ekind",
    "taida_list_map_k",
    "taida_list_fold",
    "taida_list_foldr",
    "taida_list_find_index_lax",
    "taida_polymorphic_index_of_lax",
    "taida_polymorphic_last_index_of_lax",
    "taida_set_add_tagged",
    "taida_collection_remove_tagged",
    "taida_stdout_display_string",
    "taida_typeis_named",
    "taida_primitive_tag_match",
    "taida_debug_bool",
    "taida_debug_float",
    "taida_debug_int",
    "taida_debug_polymorphic",
    "taida_debug_str",
];

/// Calls that take over the reference of their item argument (index 1):
/// the new list stores it without a retain and releases it later.
const ITEM_TAKING_CALLS: &[&str] = &[
    "taida_list_append_k",
    "taida_list_prepend",
    "taida_list_append_consume_k",
];

/// Entry point: runs the pass over every function of `module`.
pub fn insert_release_for_dead_bindings(module: &mut IrModule) {
    let facts: Vec<Facts> = module.functions.iter().map(Facts::collect).collect();
    let fresh_returning = fresh_returning_functions(&module.functions, &facts);
    let storing = storing_params(&module.functions, &facts);
    for (func, facts) in module.functions.iter_mut().zip(&facts) {
        process_function(func, facts, &fresh_returning, &storing);
    }
}

/// How an IR variable got its value. The lowering allocates a new variable
/// for every definition, so each variable has exactly one origin
/// (parameters have none).
#[derive(Debug, Clone)]
enum Origin {
    /// Numeric / Bool constants, function addresses and ownership bits.
    Inert,
    /// A string literal: static storage, releasing it is a no-op.
    StaticStr,
    /// `UseVar`: an alias of whatever the name is bound to.
    Alias(String),
    PackNew,
    PackGet(IrVar),
    Call(String, Vec<IrVar>),
    CallUser(String, Vec<IrVar>),
    CallIndirect(IrVar, Vec<IrVar>),
    Closure(Vec<String>),
    /// `CondBranch`: one of the arm results, each with whether its arm can
    /// complete (an arm that ends in `TailCall` / `Return` never yields
    /// its placeholder result).
    Branch(Vec<(IrVar, bool)>),
    Global,
}

/// Flow-insensitive facts about one function body (all arms included).
#[derive(Debug, Default)]
struct Facts {
    params: Vec<String>,
    origins: HashMap<IrVar, Origin>,
    /// Every value bound to a name: parameters (their IR variable) and
    /// `DefVar`s anywhere in the function.
    bindings: HashMap<String, Vec<IrVar>>,
    /// How many `DefVar`s bind each value.
    bind_count: HashMap<IrVar, usize>,
    /// Values stored into each pack with `PackSet`.
    stores: HashMap<IrVar, Vec<IrVar>>,
    /// How often each value is stored into a pack or list literal, and how
    /// often it is retained (the lowering retains a heap value it stores
    /// whenever it knows the value's kind).
    store_count: HashMap<IrVar, usize>,
    retain_count: HashMap<IrVar, usize>,
    /// Operands of `Return`.
    returns: Vec<IrVar>,
    /// Operands that leave the current activation: `Return`, `TailCall`
    /// (the next iteration) and `GlobalSet`.
    roots: Vec<IrVar>,
    /// Argument lists of the self tail calls.
    tail_calls: Vec<Vec<IrVar>>,
    /// Whether the body runs inside an iteration-scope arena watermark.
    iter_scope: bool,
}

impl Facts {
    fn collect(func: &IrFunction) -> Self {
        let mut facts = Facts {
            params: func.params.clone(),
            ..Facts::default()
        };
        for (index, name) in func.params.iter().enumerate() {
            facts
                .bindings
                .entry(name.clone())
                .or_default()
                .push(index as IrVar);
        }
        facts.scan(&func.body);
        facts
    }

    fn scan(&mut self, body: &[IrInst]) {
        for inst in body {
            match inst {
                IrInst::ConstInt(dst, _)
                | IrInst::ConstFloat(dst, _)
                | IrInst::ConstBool(dst, _)
                | IrInst::FuncAddr(dst, _)
                | IrInst::ParamOwned(dst, _) => {
                    self.origins.insert(*dst, Origin::Inert);
                }
                IrInst::ConstStr(dst, _) => {
                    self.origins.insert(*dst, Origin::StaticStr);
                }
                IrInst::DefVar(name, src) => {
                    self.bindings.entry(name.clone()).or_default().push(*src);
                    *self.bind_count.entry(*src).or_default() += 1;
                }
                IrInst::UseVar(dst, name) => {
                    self.origins.insert(*dst, Origin::Alias(name.clone()));
                }
                IrInst::PackNew(dst, _) => {
                    self.origins.insert(*dst, Origin::PackNew);
                }
                IrInst::PackSet(pack, _, value) => {
                    self.stores.entry(*pack).or_default().push(*value);
                    *self.store_count.entry(*value).or_default() += 1;
                }
                IrInst::PackSetTag(..) => {}
                IrInst::PackGet(dst, pack, _) => {
                    self.origins.insert(*dst, Origin::PackGet(*pack));
                }
                IrInst::Call(dst, name, args) => {
                    if name == "taida_list_push" && args.len() == 2 {
                        *self.store_count.entry(args[1]).or_default() += 1;
                    }
                    if name == "taida_str_retain" && args.len() == 1 {
                        *self.retain_count.entry(args[0]).or_default() += 1;
                    }
                    if name == "taida_arena_iter_enter" {
                        self.iter_scope = true;
                    }
                    self.origins
                        .insert(*dst, Origin::Call(name.clone(), args.clone()));
                }
                IrInst::CallUser(dst, name, args) => {
                    self.origins
                        .insert(*dst, Origin::CallUser(name.clone(), args.clone()));
                }
                IrInst::CallIndirect(dst, callee, args) => {
                    self.origins
                        .insert(*dst, Origin::CallIndirect(*callee, args.clone()));
                }
                IrInst::MakeClosure(dst, _, captures) => {
                    self.origins.insert(*dst, Origin::Closure(captures.clone()));
                }
                IrInst::CondBranch(dst, arms) => {
                    let results = arms
                        .iter()
                        .map(|arm| {
                            let completes = !arm
                                .body
                                .iter()
                                .any(|i| matches!(i, IrInst::TailCall(_) | IrInst::Return(_)));
                            (arm.result, completes)
                        })
                        .collect();
                    self.origins.insert(*dst, Origin::Branch(results));
                    for arm in arms {
                        self.scan(&arm.body);
                    }
                }
                IrInst::Retain(value) => {
                    *self.retain_count.entry(*value).or_default() += 1;
                }
                IrInst::Release(_)
                | IrInst::ReleaseAuto(_)
                | IrInst::ReleaseParamIfOwned(_)
                | IrInst::SetParamOwned(_, _) => {}
                IrInst::TailCall(args) => {
                    self.roots.extend(args);
                    self.tail_calls.push(args.clone());
                }
                IrInst::GlobalSet(_, value) => self.roots.push(*value),
                IrInst::GlobalGet(dst, _) => {
                    self.origins.insert(*dst, Origin::Global);
                }
                IrInst::Return(value) => {
                    self.returns.push(*value);
                    self.roots.push(*value);
                }
            }
        }
    }

    fn is_static_str(&self, var: IrVar) -> bool {
        matches!(self.origins.get(&var), Some(Origin::StaticStr))
    }

    /// Whether `var` holds a fresh reference created in this function (see
    /// the module docs). The answer covers the object itself; whether a
    /// particular binding may give that reference up is `owns`.
    fn is_fresh(&self, var: IrVar, fresh_returning: &HashSet<String>) -> bool {
        self.is_fresh_at(var, fresh_returning, 0)
    }

    fn is_fresh_at(&self, var: IrVar, fresh_returning: &HashSet<String>, depth: usize) -> bool {
        // Branch nesting is shallow in practice; the bound only keeps a
        // pathological input from recursing without limit.
        if depth > 64 {
            return false;
        }
        let mut var = var;
        loop {
            match self.origins.get(&var) {
                // A list literal is `taida_list_new` followed by one
                // `taida_list_push` per element; each push hands back the
                // (possibly reallocated) list it was given.
                Some(Origin::Call(name, args)) if name == "taida_list_push" => match args.first() {
                    Some(&list) => var = list,
                    None => return false,
                },
                Some(Origin::PackNew) | Some(Origin::Closure(_)) => return true,
                Some(Origin::Call(name, _)) => {
                    return FRESH_RESULT_CALLS.contains(&name.as_str());
                }
                Some(Origin::CallUser(name, _)) => return fresh_returning.contains(name),
                Some(Origin::Branch(results)) => {
                    // Every arm that can complete yields a fresh value or a
                    // string literal (releasing one is a no-op), and at
                    // least one yields a fresh value.
                    let mut any_fresh = false;
                    for &(result, completes) in results {
                        if !completes {
                            continue;
                        }
                        if self.is_fresh_at(result, fresh_returning, depth + 1) {
                            any_fresh = true;
                        } else if !self.is_static_str(result) {
                            return false;
                        }
                    }
                    return any_fresh;
                }
                _ => return false,
            }
        }
    }

    /// Whether the single binding of `var` owns it, i.e. may release it.
    fn owns(&self, var: IrVar, fresh_returning: &HashSet<String>) -> bool {
        self.bind_count.get(&var).copied().unwrap_or(0) == 1 && self.is_fresh(var, fresh_returning)
    }

    /// Whether returning `var` hands the caller a reference of its own:
    /// a fresh value, a string literal (releasing it is a no-op), or a
    /// local binding that owns a fresh value (the function-end release
    /// skips names the return value can reach, and a referenced binding is
    /// never released early).
    fn returns_owned(&self, var: IrVar, fresh_returning: &HashSet<String>) -> bool {
        if self.is_static_str(var) || self.is_fresh(var, fresh_returning) {
            return true;
        }
        let Some(Origin::Alias(name)) = self.origins.get(&var) else {
            return false;
        };
        if self.params.contains(name) {
            return false;
        }
        match self.bindings.get(name).map(Vec::as_slice) {
            Some([source]) => self.owns(*source, fresh_returning),
            _ => false,
        }
    }

    /// Names whose value may still be reachable after the function (or
    /// the current loop iteration) is left: through a returned value, a
    /// tail-call argument or a global, including through closures that
    /// capture the name and packs, lists or call results that hold it.
    fn escaping_names(&self) -> HashSet<String> {
        let mut seen: HashSet<IrVar> = HashSet::new();
        let mut names: HashSet<String> = HashSet::new();
        let mut work: Vec<IrVar> = self.roots.clone();
        while let Some(var) = work.pop() {
            if !seen.insert(var) {
                continue;
            }
            if let Some(stored) = self.stores.get(&var) {
                work.extend(stored);
            }
            match self.origins.get(&var) {
                Some(Origin::Alias(name)) => self.reach_name(name, &mut names, &mut work),
                Some(Origin::PackGet(pack)) => work.push(*pack),
                Some(Origin::Call(name, args)) => {
                    if !SCALAR_RESULT_CALLS.contains(&name.as_str()) {
                        work.extend(args);
                    }
                }
                Some(Origin::CallUser(_, args)) => work.extend(args),
                Some(Origin::CallIndirect(callee, args)) => {
                    work.push(*callee);
                    work.extend(args);
                }
                Some(Origin::Closure(captures)) => {
                    for name in captures {
                        self.reach_name(name, &mut names, &mut work);
                    }
                }
                Some(Origin::Branch(results)) => work.extend(results.iter().map(|(r, _)| *r)),
                Some(Origin::Inert)
                | Some(Origin::StaticStr)
                | Some(Origin::PackNew)
                | Some(Origin::Global)
                | None => {}
            }
        }
        names
    }

    fn reach_name(&self, name: &str, names: &mut HashSet<String>, work: &mut Vec<IrVar>) {
        if names.insert(name.to_string())
            && let Some(values) = self.bindings.get(name)
        {
            work.extend(values);
        }
    }

    /// A store of `value` comes with a retain (the lowering's
    /// retain-on-store), so the container holds a reference of its own.
    fn store_is_retained(&self, value: IrVar) -> bool {
        self.retain_count.get(&value).copied().unwrap_or(0)
            >= self.store_count.get(&value).copied().unwrap_or(0)
    }
}

/// How a value refers to another value's object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    /// It is the same object, or a value held inside it and kept alive by
    /// its reference (an alias, a branch result, a list the push
    /// reallocated, a field read, an unmold).
    Same,
    /// It points at the object, or into it, without a reference of its
    /// own and independently of it (a closure capture, a store without a
    /// retain, the result of a call that may hand back an argument).
    Borrow,
    /// It holds a reference of its own to the object, or its own
    /// references to copies of the object's contents.
    Owned,
}

/// Which values refer to which, for one function.
#[derive(Debug, Default)]
struct Graph {
    /// `preds[x]`: the objects `x` refers to, and how.
    preds: HashMap<IrVar, Vec<(IrVar, Edge)>>,
    /// `succs[y]`: the values that refer to `y`'s object, and how.
    succs: HashMap<IrVar, Vec<(IrVar, Edge)>>,
    /// Values something may keep past the current activation: arguments of
    /// calls that are not known to only read them, arguments a callee
    /// keeps, and globals.
    store_roots: Vec<IrVar>,
    /// Values whose reference a call took over.
    consumed: HashSet<IrVar>,
}

impl Graph {
    fn build(func: &IrFunction, facts: &Facts, storing: &HashMap<String, Vec<bool>>) -> Self {
        let mut graph = Graph::default();
        graph.scan(&func.body, facts, storing);
        // A call takes over the object, not just the variable naming it:
        // the value an alias was read from is consumed as well.
        let mut work: Vec<IrVar> = graph.consumed.iter().copied().collect();
        while let Some(var) = work.pop() {
            for &(pred, kind) in graph.preds.get(&var).into_iter().flatten() {
                if kind == Edge::Same && graph.consumed.insert(pred) {
                    work.push(pred);
                }
            }
        }
        graph
    }

    fn edge(&mut self, from: IrVar, to: IrVar, kind: Edge) {
        self.preds.entry(to).or_default().push((from, kind));
        self.succs.entry(from).or_default().push((to, kind));
    }

    fn scan(&mut self, body: &[IrInst], facts: &Facts, storing: &HashMap<String, Vec<bool>>) {
        for inst in body {
            match inst {
                IrInst::UseVar(dst, name) => {
                    for &source in facts.bindings.get(name).into_iter().flatten() {
                        self.edge(source, *dst, Edge::Same);
                    }
                }
                // A field read is kept alive by the pack's reference.
                IrInst::PackGet(dst, pack, _) => self.edge(*pack, *dst, Edge::Same),
                IrInst::PackSet(pack, _, value) => {
                    let kind = if facts.store_is_retained(*value) {
                        Edge::Owned
                    } else {
                        Edge::Borrow
                    };
                    self.edge(*value, *pack, kind);
                }
                IrInst::Call(dst, name, args) => self.scan_call(*dst, name, args, facts),
                IrInst::CallUser(dst, name, args) => {
                    let kept = storing.get(name);
                    for (index, &arg) in args.iter().enumerate() {
                        self.edge(arg, *dst, Edge::Borrow);
                        let keeps = kept.map(|k| k.get(index).copied().unwrap_or(true));
                        if keeps.unwrap_or(true) {
                            self.store_roots.push(arg);
                        }
                    }
                }
                IrInst::CallIndirect(dst, callee, args) => {
                    self.edge(*callee, *dst, Edge::Borrow);
                    self.store_roots.push(*callee);
                    for &arg in args {
                        self.edge(arg, *dst, Edge::Borrow);
                        self.store_roots.push(arg);
                    }
                }
                IrInst::MakeClosure(dst, _, captures) => {
                    for name in captures {
                        for &source in facts.bindings.get(name).into_iter().flatten() {
                            self.edge(source, *dst, Edge::Borrow);
                        }
                    }
                }
                IrInst::CondBranch(dst, arms) => {
                    for arm in arms {
                        self.edge(arm.result, *dst, Edge::Same);
                        self.scan(&arm.body, facts, storing);
                    }
                }
                IrInst::GlobalSet(_, value) => self.store_roots.push(*value),
                _ => {}
            }
        }
    }

    fn scan_call(&mut self, dst: IrVar, name: &str, args: &[IrVar], facts: &Facts) {
        if SCALAR_RESULT_CALLS.contains(&name) {
            return;
        }
        if name == "taida_list_push" && args.len() == 2 {
            self.edge(args[0], dst, Edge::Same);
            self.consumed.insert(args[0]);
            let kind = if facts.store_is_retained(args[1]) {
                Edge::Owned
            } else {
                Edge::Borrow
            };
            self.edge(args[1], dst, kind);
            return;
        }
        if name == "taida_hashmap_set_consume" && args.len() == 5 {
            self.edge(args[0], dst, Edge::Same);
            self.consumed.insert(args[0]);
            self.edge(args[2], dst, Edge::Owned);
            self.edge(args[3], dst, Edge::Owned);
            return;
        }
        if ITEM_TAKING_CALLS.contains(&name) && args.len() >= 2 {
            // The consume variant may extend the list in place.
            let list_kind = if name == "taida_list_append_consume_k" {
                Edge::Same
            } else {
                Edge::Owned
            };
            self.edge(args[0], dst, list_kind);
            self.edge(args[1], dst, Edge::Owned);
            self.consumed.insert(args[1]);
            return;
        }
        if COPY_RESULT_CALLS.contains(&name) {
            return;
        }
        if FRESH_RESULT_CALLS.contains(&name) {
            for &arg in args {
                self.edge(arg, dst, Edge::Owned);
            }
            return;
        }
        if ACCESSOR_CALLS.contains(&name) {
            for &arg in args {
                self.edge(arg, dst, Edge::Same);
            }
            return;
        }
        for &arg in args {
            self.edge(arg, dst, Edge::Borrow);
        }
        if !READ_ONLY_ARG_CALLS.contains(&name) {
            self.store_roots.extend(args);
        }
    }

    /// Values whose object must stay alive while any of `roots` is still
    /// in use: the roots themselves, what they are aliases of, and what
    /// they (or anything that holds them) point at without a reference of
    /// their own. Returns (objects, dependencies): `objects` must not be
    /// freed, `dependencies` must keep what they borrow alive.
    fn needed(&self, roots: impl IntoIterator<Item = IrVar>) -> (HashSet<IrVar>, HashSet<IrVar>) {
        let mut objects: HashSet<IrVar> = HashSet::new();
        let mut deps: HashSet<IrVar> = HashSet::new();
        let mut work: Vec<(IrVar, bool)> = roots.into_iter().map(|root| (root, true)).collect();
        while let Some((var, object)) = work.pop() {
            if object {
                if !objects.insert(var) {
                    continue;
                }
                work.push((var, false));
            } else if !deps.insert(var) {
                continue;
            }
            for &(pred, kind) in self.preds.get(&var).into_iter().flatten() {
                match (kind, object) {
                    (Edge::Same, _) => work.push((pred, object)),
                    (Edge::Borrow, false) => work.push((pred, true)),
                    (Edge::Owned, false) => work.push((pred, false)),
                    (Edge::Borrow, true) | (Edge::Owned, true) => {}
                }
            }
        }
        (objects, deps)
    }

    /// Values that need `var`'s object alive: its aliases, anything that
    /// borrows from one of them, and whatever holds those.
    fn needers(&self, var: IrVar) -> HashSet<IrVar> {
        let mut aliases: HashSet<IrVar> = HashSet::new();
        let mut work = vec![var];
        while let Some(v) = work.pop() {
            if !aliases.insert(v) {
                continue;
            }
            for &(succ, kind) in self.succs.get(&v).into_iter().flatten() {
                if kind == Edge::Same {
                    work.push(succ);
                }
            }
        }
        let mut holders: HashSet<IrVar> = HashSet::new();
        let mut work: Vec<IrVar> = aliases
            .iter()
            .flat_map(|v| self.succs.get(v).into_iter().flatten())
            .filter(|(_, kind)| *kind == Edge::Borrow)
            .map(|(succ, _)| *succ)
            .collect();
        while let Some(v) = work.pop() {
            if !holders.insert(v) {
                continue;
            }
            for &(succ, _) in self.succs.get(&v).into_iter().flatten() {
                work.push(succ);
            }
        }
        aliases.remove(&var);
        aliases.extend(holders);
        aliases
    }
}

/// Functions whose every `Return` hands the caller a reference of its own
/// (greatest fixpoint: a recursive call counts as fresh as long as every
/// base case is).
fn fresh_returning_functions(functions: &[IrFunction], facts: &[Facts]) -> HashSet<String> {
    let mut fresh: HashSet<String> = functions.iter().map(|f| f.name.clone()).collect();
    loop {
        let mut changed = false;
        for (func, facts) in functions.iter().zip(facts) {
            if !fresh.contains(&func.name) {
                continue;
            }
            let returns_fresh = !facts.returns.is_empty()
                && facts
                    .returns
                    .iter()
                    .all(|&var| facts.returns_owned(var, &fresh));
            if !returns_fresh {
                fresh.remove(&func.name);
                changed = true;
            }
        }
        if !changed {
            return fresh;
        }
    }
}

/// For every function, which parameters it may keep after it returns
/// (other than through its return value): stored by a runtime call that
/// keeps its arguments, captured by something kept that way, or passed on
/// to a callee that keeps it. Least fixpoint.
fn storing_params(functions: &[IrFunction], facts: &[Facts]) -> HashMap<String, Vec<bool>> {
    let mut storing: HashMap<String, Vec<bool>> = functions
        .iter()
        .map(|f| (f.name.clone(), vec![false; f.params.len()]))
        .collect();
    loop {
        let mut changed = false;
        for (func, facts) in functions.iter().zip(facts) {
            if func.params.is_empty() {
                continue;
            }
            let graph = Graph::build(func, facts, &storing);
            let own = storing[&func.name].clone();
            let mut roots = graph.store_roots.clone();
            for args in &facts.tail_calls {
                for (index, &arg) in args.iter().enumerate() {
                    if own.get(index).copied().unwrap_or(false) {
                        roots.push(arg);
                    }
                }
            }
            let (_, deps) = graph.needed(roots);
            let flags = storing
                .get_mut(&func.name)
                .expect("every function has flags");
            for (index, flag) in flags.iter_mut().enumerate() {
                if !*flag && deps.contains(&(index as IrVar)) {
                    *flag = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return storing;
        }
    }
}

fn process_function(
    func: &mut IrFunction,
    facts: &Facts,
    fresh_returning: &HashSet<String>,
    storing: &HashMap<String, Vec<bool>>,
) {
    drop_unsafe_function_end_releases(&mut func.body, facts, fresh_returning);

    // Names still released at function exit by the lowering's
    // `[UseVar(t, name), Release(t)]` pairs. A binding of such a name is
    // never released on the path that reaches those pairs as well, or the
    // value would be released twice.
    let function_end_releases = collect_function_end_release_names(&func.body);

    let ctx = BodyCtx {
        facts,
        fresh_returning,
        function_end_releases: &function_end_releases,
    };
    let mut released = HashSet::new();
    process_body(&mut func.body, None, &ctx, &mut released);

    // `_taida_main` runs once and ends the process; an iteration-scope
    // loop already rewinds its arena at every back-edge.
    if func.name == "_taida_main" || facts.iter_scope {
        return;
    }
    release_at_scope_exits(
        func,
        facts,
        fresh_returning,
        storing,
        &function_end_releases,
        &released,
    );
}

/// Remove the function-end `[UseVar(t, name), Release(t)]` pairs that
/// would free a value still in use.
///
/// - `name` is bound only inside a condition arm: the binding does not
///   dominate the function end (the native emitter cannot even read it
///   there), and the arm may yield the value as its result.
/// - `name`'s value can outlive the function: it is reachable from a
///   returned value (for example captured by a returned closure).
/// - `name` does not own its value: `r.map(f)` hands back `r` itself when
///   `r` is an error Result (or a pending Async), so releasing it would free
///   the caller's value.
fn drop_unsafe_function_end_releases(
    body: &mut Vec<IrInst>,
    facts: &Facts,
    fresh_returning: &HashSet<String>,
) {
    let top_level_bound: HashSet<&str> = body
        .iter()
        .filter_map(|inst| match inst {
            IrInst::DefVar(name, _) => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let escaping = facts.escaping_names();
    let mut drop_at: Vec<usize> = Vec::new();
    for (i, window) in body.windows(2).enumerate() {
        if let (IrInst::UseVar(used, name), IrInst::Release(released)) = (&window[0], &window[1])
            && used == released
            && (!top_level_bound.contains(name.as_str())
                || escaping.contains(name)
                || !owns_every_binding(facts, name, fresh_returning))
        {
            drop_at.push(i);
        }
    }
    for i in drop_at.into_iter().rev() {
        body.drain(i..i + 2);
    }
}

fn owns_every_binding(facts: &Facts, name: &str, fresh_returning: &HashSet<String>) -> bool {
    facts
        .bindings
        .get(name)
        .is_some_and(|values| values.iter().all(|&v| facts.owns(v, fresh_returning)))
}

/// Walk the top-level body and find names that already have a
/// function-end `[UseVar(_, name), Release(_)]` pair.
fn collect_function_end_release_names(body: &[IrInst]) -> HashSet<String> {
    let mut names = HashSet::new();
    for window in body.windows(2) {
        if let (IrInst::UseVar(uv, name), IrInst::Release(rv)) = (&window[0], &window[1])
            && uv == rv
        {
            names.insert(name.clone());
        }
    }
    names
}

struct BodyCtx<'a> {
    facts: &'a Facts,
    fresh_returning: &'a HashSet<String>,
    function_end_releases: &'a HashSet<String>,
}

/// Insert `ReleaseAuto` after dead bindings of `body`. `arm_result` is the
/// value the enclosing condition arm yields (`None` for a function body):
/// a binding of that value is the arm's result, not a dead binding.
/// Every value released here is added to `released`.
fn process_body(
    body: &mut Vec<IrInst>,
    arm_result: Option<IrVar>,
    ctx: &BodyCtx<'_>,
    released: &mut HashSet<IrVar>,
) {
    // Nested arms first, so their insertions don't shift the indices
    // computed for this body below.
    for inst in body.iter_mut() {
        if let IrInst::CondBranch(_, arms) = inst {
            for arm in arms.iter_mut() {
                let result = arm.result;
                process_body(&mut arm.body, Some(result), ctx, released);
            }
        }
    }

    let mut insertions: Vec<(usize, IrVar)> = Vec::new();
    // A rebound name ends the earlier binding's lifetime at the second
    // `DefVar`; only the first binding of a name is considered.
    let mut seen_defs: HashSet<String> = HashSet::new();
    for (i, inst) in body.iter().enumerate() {
        let IrInst::DefVar(name, value_var) = inst else {
            continue;
        };
        let first_binding = seen_defs.insert(name.clone());
        if !first_binding || ctx.function_end_releases.contains(name) {
            continue;
        }
        if arm_result == Some(*value_var) {
            continue;
        }
        if !ctx.facts.owns(*value_var, ctx.fresh_returning) {
            continue;
        }
        if !is_referenced_after(body, i, name, *value_var) && !name_referenced_before(body, i, name)
        {
            insertions.push((i + 1, *value_var));
        }
    }

    // Apply insertions in reverse order so earlier indices stay valid.
    for (idx, var) in insertions.into_iter().rev() {
        released.insert(var);
        body.insert(idx, IrInst::ReleaseAuto(var));
    }
}

/// `body[start_idx + 1..]` および各 nested CondArm.body 内に
/// `name` への UseVar 参照、または `value_var` 自体を引数に取る命令が
/// 存在するかを返す。
fn is_referenced_after(body: &[IrInst], start_idx: usize, name: &str, value_var: IrVar) -> bool {
    for inst in body.iter().skip(start_idx + 1) {
        if inst_references(inst, name, value_var) {
            return true;
        }
    }
    false
}

/// `body[..before_idx]` 内に `name` を DefVar / UseVar していたかを返す。
/// rebinding/早期 capture 検出に使う。
fn name_referenced_before(body: &[IrInst], before_idx: usize, name: &str) -> bool {
    for inst in body.iter().take(before_idx) {
        match inst {
            IrInst::DefVar(n, _) | IrInst::UseVar(_, n) if n == name => return true,
            IrInst::MakeClosure(_, _, captures) if captures.iter().any(|c| c == name) => {
                return true;
            }
            IrInst::CondBranch(_, arms) => {
                for arm in arms {
                    if name_referenced_before(&arm.body, arm.body.len(), name) {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// `inst` が `name` を読む or `value_var` を引数に取るかを返す。
/// nested CondBranch arms も再帰的にスキャンする。
fn inst_references(inst: &IrInst, name: &str, value_var: IrVar) -> bool {
    match inst {
        IrInst::DefVar(_, src) => *src == value_var,
        IrInst::UseVar(_, n) => n == name,
        IrInst::MakeClosure(_, _, captures) => captures.iter().any(|c| c == name),
        IrInst::Retain(v) | IrInst::Release(v) | IrInst::ReleaseAuto(v) | IrInst::Return(v) => {
            *v == value_var
        }
        IrInst::Call(_, _, args) | IrInst::CallUser(_, _, args) | IrInst::TailCall(args) => {
            args.contains(&value_var)
        }
        IrInst::CallIndirect(_, fn_var, args) => *fn_var == value_var || args.contains(&value_var),
        IrInst::PackNew(_, _) => false,
        IrInst::PackSet(p, _, v) => *p == value_var || *v == value_var,
        IrInst::PackSetTag(p, _, _) => *p == value_var,
        IrInst::PackGet(_, p, _) => *p == value_var,
        IrInst::FuncAddr(_, _) => false,
        IrInst::CondBranch(result, arms) => {
            // The CondBranch's result var carries a value that may or
            // may not be ours; arms compute it. If any arm's result is
            // value_var (= we're returning the binding through a
            // branch) or any arm body references the name/value, we
            // count this as a reference.
            if *result == value_var {
                return true;
            }
            for arm in arms {
                if let Some(c) = arm.condition
                    && c == value_var
                {
                    return true;
                }
                if arm.result == value_var {
                    return true;
                }
                for inner in &arm.body {
                    if inst_references(inner, name, value_var) {
                        return true;
                    }
                }
            }
            false
        }
        IrInst::GlobalSet(_, v) => *v == value_var,
        IrInst::GlobalGet(_, _) => false,
        IrInst::ConstInt(_, _)
        | IrInst::ConstFloat(_, _)
        | IrInst::ConstStr(_, _)
        | IrInst::ConstBool(_, _)
        | IrInst::ReleaseParamIfOwned(_)
        | IrInst::SetParamOwned(_, _)
        | IrInst::ParamOwned(_, _) => false,
    }
}

// ── Releases at scope exits ─────────────────────────────────────────

/// The variable an instruction defines, if any.
fn defined_var(inst: &IrInst) -> Option<IrVar> {
    match inst {
        IrInst::ConstInt(dst, _)
        | IrInst::ConstFloat(dst, _)
        | IrInst::ConstStr(dst, _)
        | IrInst::ConstBool(dst, _)
        | IrInst::UseVar(dst, _)
        | IrInst::PackNew(dst, _)
        | IrInst::PackGet(dst, _, _)
        | IrInst::Call(dst, _, _)
        | IrInst::CallUser(dst, _, _)
        | IrInst::CallIndirect(dst, _, _)
        | IrInst::MakeClosure(dst, _, _)
        | IrInst::FuncAddr(dst, _)
        | IrInst::CondBranch(dst, _)
        | IrInst::GlobalGet(dst, _)
        | IrInst::ParamOwned(dst, _) => Some(*dst),
        _ => None,
    }
}

/// The variables an instruction reads (not descending into arms).
fn operand_vars(inst: &IrInst) -> Vec<IrVar> {
    match inst {
        IrInst::DefVar(_, v)
        | IrInst::Retain(v)
        | IrInst::Release(v)
        | IrInst::ReleaseAuto(v)
        | IrInst::Return(v)
        | IrInst::GlobalSet(_, v)
        | IrInst::PackSetTag(v, _, _)
        | IrInst::PackGet(_, v, _) => vec![*v],
        IrInst::PackSet(p, _, v) => vec![*p, *v],
        IrInst::Call(_, _, args) | IrInst::CallUser(_, _, args) | IrInst::TailCall(args) => {
            args.clone()
        }
        IrInst::CallIndirect(_, callee, args) => {
            let mut vars = vec![*callee];
            vars.extend(args);
            vars
        }
        IrInst::CondBranch(_, arms) => arms
            .iter()
            .flat_map(|arm| arm.condition.into_iter().chain(std::iter::once(arm.result)))
            .collect(),
        _ => Vec::new(),
    }
}

/// Everything an instruction subtree defines, reads, or names.
#[derive(Default)]
struct Footprint {
    defined: HashSet<IrVar>,
    read: HashMap<IrVar, usize>,
    names_used: HashMap<String, usize>,
}

impl Footprint {
    fn of(body: &[IrInst]) -> Self {
        let mut footprint = Footprint::default();
        footprint.add(body);
        footprint
    }

    fn add(&mut self, body: &[IrInst]) {
        for inst in body {
            if let Some(dst) = defined_var(inst) {
                self.defined.insert(dst);
            }
            for var in operand_vars(inst) {
                *self.read.entry(var).or_default() += 1;
            }
            match inst {
                IrInst::UseVar(_, name) => *self.names_used.entry(name.clone()).or_default() += 1,
                IrInst::MakeClosure(_, _, captures) => {
                    for name in captures {
                        *self.names_used.entry(name.clone()).or_default() += 1;
                    }
                }
                IrInst::CondBranch(_, arms) => {
                    for arm in arms {
                        self.add(&arm.body);
                    }
                }
                _ => {}
            }
        }
    }
}

struct ExitCtx<'a> {
    facts: &'a Facts,
    graph: &'a Graph,
    /// Owned values this pass may release, with the name that binds each
    /// (if any).
    candidates: HashMap<IrVar, Option<String>>,
    function_end_releases: &'a HashSet<String>,
    /// Objects kept by the runtime or a global, independently of any exit.
    stored_objects: HashSet<IrVar>,
    /// Parameters whose loop ownership is tracked.
    tracked_params: BTreeSet<usize>,
    /// Self tail calls whose parameter `j` gets a consuming map update:
    /// `(tail call operands, j)`.
    consume_params: HashSet<(Vec<IrVar>, usize)>,
    /// Whole-function footprint (for "used outside this arm" checks).
    whole: Footprint,
    /// The names that bind each value.
    names_of: HashMap<IrVar, Vec<String>>,
}

fn release_at_scope_exits(
    func: &mut IrFunction,
    facts: &Facts,
    fresh_returning: &HashSet<String>,
    storing: &HashMap<String, Vec<bool>>,
    function_end_releases: &HashSet<String>,
    released: &HashSet<IrVar>,
) {
    // Loop parameters: rewrite map updates into consuming ones first; the
    // rewrite changes the IR, so the facts are collected again after it.
    let tracks_params = !facts.tail_calls.is_empty() && !func.append_consume_owned;
    let mut consume_params: HashSet<(Vec<IrVar>, usize)> = HashSet::new();
    if tracks_params {
        let graph = Graph::build(func, facts, storing);
        let (stored_objects, _) = graph.needed(graph.store_roots.clone());
        let mut next_var = func.next_var;
        rewrite_consuming_map_updates(
            &mut func.body,
            facts,
            &graph,
            &stored_objects,
            &mut consume_params,
            &mut next_var,
        );
        func.next_var = next_var;
    }
    let refreshed;
    let facts = if consume_params.is_empty() {
        facts
    } else {
        refreshed = Facts::collect(func);
        &refreshed
    };
    let graph = Graph::build(func, facts, storing);
    let (stored_objects, _) = graph.needed(graph.store_roots.clone());

    let loop_params = func.params.len();
    let mut names_of: HashMap<IrVar, Vec<String>> = HashMap::new();
    for (name, values) in &facts.bindings {
        for &value in values {
            names_of.entry(value).or_default().push(name.clone());
        }
    }
    let mut candidates: HashMap<IrVar, Option<String>> = HashMap::new();
    for &var in facts.origins.keys() {
        if (var as usize) < loop_params
            || released.contains(&var)
            || graph.consumed.contains(&var)
            || facts.bind_count.get(&var).copied().unwrap_or(0) > 1
            || !facts.is_fresh(var, fresh_returning)
        {
            continue;
        }
        // Candidates are bound at most once.
        let name = names_of.get(&var).and_then(|names| names.first().cloned());
        candidates.insert(var, name);
    }

    let mut tracked_params = BTreeSet::new();
    if tracks_params {
        for args in &facts.tail_calls {
            for (index, &arg) in args.iter().enumerate().take(loop_params) {
                if consume_params.contains(&(args.clone(), index))
                    || (candidates.contains_key(&arg) && !is_param_passthrough(facts, arg, index))
                {
                    tracked_params.insert(index);
                }
            }
        }
    }

    let ctx = ExitCtx {
        facts,
        graph: &graph,
        candidates,
        function_end_releases,
        stored_objects,
        tracked_params,
        consume_params,
        whole: Footprint::of(&func.body),
        names_of,
    };
    walk_exits(&mut func.body, &[], None, true, &ctx);
}

/// `arg` passes parameter `index` on unchanged.
fn is_param_passthrough(facts: &Facts, arg: IrVar, index: usize) -> bool {
    if arg as usize == index {
        return true;
    }
    let Some(Origin::Alias(name)) = facts.origins.get(&arg) else {
        return false;
    };
    facts.params.get(index) == Some(name)
        && facts.bindings.get(name).map(Vec::as_slice) == Some(&[index as IrVar][..])
}

/// Rewrite `m.set(k, v)` into the consuming update when its result is what
/// a self tail call passes for the map parameter `m`, nothing reaches the
/// old map after the tail call, and nothing between the update and the
/// tail call reads it (the update may modify it in place).
fn rewrite_consuming_map_updates(
    body: &mut Vec<IrInst>,
    facts: &Facts,
    graph: &Graph,
    stored_objects: &HashSet<IrVar>,
    consume_params: &mut HashSet<(Vec<IrVar>, usize)>,
    next_var: &mut IrVar,
) {
    for inst in body.iter_mut() {
        if let IrInst::CondBranch(_, arms) = inst {
            for arm in arms.iter_mut() {
                rewrite_consuming_map_updates(
                    &mut arm.body,
                    facts,
                    graph,
                    stored_objects,
                    consume_params,
                    next_var,
                );
            }
        }
    }
    let Some(IrInst::TailCall(args)) = body
        .iter()
        .find(|i| matches!(i, IrInst::TailCall(_)))
        .cloned()
    else {
        return;
    };
    let (objects, _) = graph.needed(args.iter().copied());
    for (index, &arg) in args.iter().enumerate().take(facts.params.len()) {
        let Some(tail_idx) = body.iter().position(|i| matches!(i, IrInst::TailCall(_))) else {
            return;
        };
        // The update must be in this body, before the tail call.
        let Some(set_idx) = body[..tail_idx].iter().position(|i| {
            matches!(i, IrInst::Call(dst, name, _)
                if *dst == arg && name == "taida_hashmap_set_immut")
        }) else {
            continue;
        };
        let IrInst::Call(_, _, set_args) = &body[set_idx] else {
            continue;
        };
        if set_args.len() != 4 || !is_param_passthrough(facts, set_args[0], index) {
            continue;
        }
        // Only this parameter position may pass the result on, and the old
        // map must not be needed after the tail call.
        let param = index as IrVar;
        if objects.contains(&param)
            || stored_objects.contains(&param)
            || args.iter().filter(|&&a| a == arg).count() != 1
        {
            continue;
        }
        let mut touching = graph.needers(param);
        touching.insert(param);
        let after = Footprint::of(&body[set_idx + 1..tail_idx]);
        let touched_after = touching
            .iter()
            .any(|v| after.defined.contains(v) || after.read.contains_key(v))
            || after.names_used.contains_key(facts.params[index].as_str());
        if touched_after {
            continue;
        }
        let flag = *next_var;
        *next_var += 1;
        let mut consume_args = set_args.clone();
        consume_args.push(flag);
        body[set_idx] = IrInst::Call(arg, "taida_hashmap_set_consume".to_string(), consume_args);
        body.insert(set_idx, IrInst::ParamOwned(flag, index));
        consume_params.insert((args.clone(), index));
    }
}

/// Insert the releases for `body`. `pending` are the owned values made
/// earlier on every path into `body` (in enclosing bodies); `arm_result`
/// is set when `body` is a condition arm.
fn walk_exits(
    body: &mut Vec<IrInst>,
    pending: &[IrVar],
    arm_result: Option<IrVar>,
    top_level: bool,
    ctx: &ExitCtx<'_>,
) {
    let mut local: Vec<IrVar> = Vec::new();
    let mut insertions: Vec<(usize, Vec<IrInst>)> = Vec::new();
    let mut exits = false;
    for (idx, inst) in body.iter_mut().enumerate() {
        match inst {
            IrInst::CondBranch(_, arms) => {
                let mut inner_pending = pending.to_vec();
                inner_pending.extend(&local);
                for arm in arms.iter_mut() {
                    let result = arm.result;
                    walk_exits(&mut arm.body, &inner_pending, Some(result), false, ctx);
                }
            }
            IrInst::TailCall(args) => {
                exits = true;
                let releases = releases_before_tail_call(pending, &local, args, ctx);
                insertions.push((idx, releases));
            }
            IrInst::Return(value) => {
                exits = true;
                let releases = releases_before_return(pending, &local, *value, top_level, ctx);
                insertions.push((idx, releases));
            }
            _ => {}
        }
        if let Some(dst) = defined_var(inst)
            && ctx.candidates.contains_key(&dst)
        {
            local.push(dst);
        }
    }
    if let Some(result) = arm_result
        && !exits
        && !local.is_empty()
    {
        let arm_footprint = Footprint::of(body);
        let releases: Vec<IrInst> = local
            .iter()
            .filter(|&&var| releasable_at_arm_end(var, result, &arm_footprint, ctx))
            .map(|&var| IrInst::ReleaseAuto(var))
            .collect();
        insertions.push((body.len(), releases));
    }
    for (idx, releases) in insertions.into_iter().rev() {
        body.splice(idx..idx, releases);
    }
}

fn releases_before_tail_call(
    pending: &[IrVar],
    local: &[IrVar],
    args: &[IrVar],
    ctx: &ExitCtx<'_>,
) -> Vec<IrInst> {
    let (objects, _) = ctx.graph.needed(args.iter().copied());
    let mut releases: Vec<IrInst> = pending
        .iter()
        .chain(local)
        .filter(|&&var| {
            !objects.contains(&var)
                && !ctx.stored_objects.contains(&var)
                && released_on_this_path_is_safe(var, false, ctx)
        })
        .map(|&var| IrInst::ReleaseAuto(var))
        .collect();
    for &index in &ctx.tracked_params {
        let Some(&arg) = args.get(index) else {
            continue;
        };
        if is_param_passthrough(ctx.facts, arg, index) {
            continue;
        }
        if ctx.consume_params.contains(&(args.to_vec(), index)) {
            // The consuming update took the loop's reference over.
            releases.push(IrInst::SetParamOwned(index, true));
            continue;
        }
        let param = index as IrVar;
        if !objects.contains(&param) && !ctx.stored_objects.contains(&param) {
            releases.push(IrInst::ReleaseParamIfOwned(index));
        }
        let owned_next = ctx.candidates.contains_key(&arg) && {
            // Nothing but this parameter position may keep the new value.
            let others = args
                .iter()
                .enumerate()
                .filter(|&(i, _)| i != index)
                .map(|(_, &a)| a);
            let (other_objects, _) = ctx.graph.needed(others);
            !other_objects.contains(&arg) && !ctx.stored_objects.contains(&arg)
        };
        releases.push(IrInst::SetParamOwned(index, owned_next));
    }
    releases
}

fn releases_before_return(
    pending: &[IrVar],
    local: &[IrVar],
    value: IrVar,
    top_level: bool,
    ctx: &ExitCtx<'_>,
) -> Vec<IrInst> {
    let (objects, _) = ctx.graph.needed([value]);
    let mut releases: Vec<IrInst> = pending
        .iter()
        .chain(local)
        .filter(|&&var| {
            !objects.contains(&var)
                && !ctx.stored_objects.contains(&var)
                && released_on_this_path_is_safe(var, top_level, ctx)
        })
        .map(|&var| IrInst::ReleaseAuto(var))
        .collect();
    for &index in &ctx.tracked_params {
        let param = index as IrVar;
        if !objects.contains(&param) && !ctx.stored_objects.contains(&param) {
            releases.push(IrInst::ReleaseParamIfOwned(index));
        }
    }
    releases
}

/// A binding of a name the function-end release pairs cover is released
/// by those pairs on the path through the end of the function body; on
/// other paths (a tail call, a return inside an arm) this pass releases
/// it, but only when that name has no other binding the pairs could
/// mean instead.
fn released_on_this_path_is_safe(
    var: IrVar,
    reaches_function_end: bool,
    ctx: &ExitCtx<'_>,
) -> bool {
    let Some(Some(name)) = ctx.candidates.get(&var) else {
        return true;
    };
    if !ctx.function_end_releases.contains(name) {
        return true;
    }
    !reaches_function_end && ctx.facts.bindings.get(name).map(Vec::len) == Some(1)
}

/// Whether an owned value made in a condition arm can be released when the
/// arm ends: nothing outside the arm (including the value the arm yields)
/// may still reach it through an alias or a borrowed pointer.
fn releasable_at_arm_end(
    var: IrVar,
    arm_result: IrVar,
    arm: &Footprint,
    ctx: &ExitCtx<'_>,
) -> bool {
    if ctx.stored_objects.contains(&var) || var == arm_result {
        return false;
    }
    if let Some(Some(name)) = ctx.candidates.get(&var)
        && (ctx.function_end_releases.contains(name) || used_outside(name, arm, ctx))
    {
        return false;
    }
    let needers = ctx.graph.needers(var);
    for needer in needers {
        if needer == arm_result
            || !arm.defined.contains(&needer)
            || ctx.stored_objects.contains(&needer)
        {
            return false;
        }
        let reads_here = arm.read.get(&needer).copied().unwrap_or(0);
        let reads_total = ctx.whole.read.get(&needer).copied().unwrap_or(0);
        if reads_total > reads_here {
            return false;
        }
        if let Some(names) = ctx.names_of.get(&needer)
            && names.iter().any(|name| used_outside(name, arm, ctx))
        {
            return false;
        }
    }
    let reads_here = arm.read.get(&var).copied().unwrap_or(0);
    let reads_total = ctx.whole.read.get(&var).copied().unwrap_or(0);
    reads_total <= reads_here
}

fn used_outside(name: &str, arm: &Footprint, ctx: &ExitCtx<'_>) -> bool {
    let here = arm.names_used.get(name).copied().unwrap_or(0);
    let total = ctx.whole.names_used.get(name).copied().unwrap_or(0);
    total > here
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_count(body: &[IrInst]) -> usize {
        let mut n = 0;
        for inst in body {
            if matches!(inst, IrInst::ReleaseAuto(_)) {
                n += 1;
            }
            if let IrInst::CondBranch(_, arms) = inst {
                for arm in arms {
                    n += release_count(&arm.body);
                }
            }
        }
        n
    }

    fn function_end_release_count(body: &[IrInst]) -> usize {
        body.iter()
            .filter(|inst| matches!(inst, IrInst::Release(_)))
            .count()
    }

    fn run(func: &mut IrFunction) {
        let mut module = IrModule::new();
        module.functions.push(func.clone());
        insert_release_for_dead_bindings(&mut module);
        *func = module.functions.pop().unwrap();
    }

    /// Linear case: `s <= expr; iter(n)` — `s` is dead after DefVar.
    #[test]
    fn dead_binding_in_linear_body_gets_release() {
        let mut func = IrFunction::new("iter".to_string());
        // simulate: v0 = call(taida_str_repeat, [arg, n]); DefVar("s", v0); v1 = call(iter, [arg]); Return(v1)
        func.body
            .push(IrInst::Call(0, "taida_str_repeat".to_string(), vec![]));
        func.body.push(IrInst::DefVar("s".to_string(), 0));
        func.body
            .push(IrInst::CallUser(1, "iter".to_string(), vec![]));
        func.body.push(IrInst::Return(1));

        run(&mut func);
        assert_eq!(
            release_count(&func.body),
            1,
            "dead binding should get one ReleaseAuto"
        );
        // The release should sit at index 2 (right after DefVar).
        assert!(matches!(func.body[2], IrInst::ReleaseAuto(0)));
    }

    /// Used after DefVar: should NOT release.
    #[test]
    fn used_binding_skipped() {
        let mut func = IrFunction::new("f".to_string());
        func.body
            .push(IrInst::Call(0, "taida_str_repeat".to_string(), vec![]));
        func.body.push(IrInst::DefVar("s".to_string(), 0));
        func.body.push(IrInst::UseVar(1, "s".to_string()));
        func.body.push(IrInst::Return(1));
        run(&mut func);
        assert_eq!(
            release_count(&func.body),
            0,
            "used binding must not be released"
        );
    }

    /// Returned binding (escapes via Return): should NOT release.
    #[test]
    fn returned_value_var_skipped() {
        let mut func = IrFunction::new("f".to_string());
        func.body
            .push(IrInst::Call(0, "taida_str_repeat".to_string(), vec![]));
        func.body.push(IrInst::DefVar("s".to_string(), 0));
        func.body.push(IrInst::Return(0));
        run(&mut func);
        assert_eq!(
            release_count(&func.body),
            0,
            "value escaping via Return must not be released"
        );
    }

    /// CondArm linear binding: `iter` recursive case.
    #[test]
    fn cond_arm_linear_binding_gets_release() {
        let mut func = IrFunction::new("iter".to_string());
        // Outer body: just CondBranch(result, [arm0, arm1])
        // arm0: ConstInt(10, 0), result=10
        // arm1: Call(20, "taida_str_repeat", []), DefVar("s", 20),
        //       Call(21, "taida_int_sub", []), TailCall([21]), result=21
        let arm0 = CondArm {
            condition: Some(99),
            body: vec![IrInst::ConstInt(10, 0)],
            result: 10,
        };
        let arm1_body = vec![
            IrInst::Call(20, "taida_str_repeat".to_string(), vec![]),
            IrInst::DefVar("s".to_string(), 20),
            IrInst::Call(21, "taida_int_sub".to_string(), vec![]),
            IrInst::TailCall(vec![21]),
        ];
        let arm1 = CondArm {
            condition: None,
            body: arm1_body,
            result: 21,
        };
        func.body.push(IrInst::CondBranch(0, vec![arm0, arm1]));

        run(&mut func);
        assert_eq!(
            release_count(&func.body),
            1,
            "arm-local dead binding should get a ReleaseAuto"
        );

        // verify it sits inside arm1.body, after DefVar
        if let IrInst::CondBranch(_, arms) = &func.body[0] {
            let arm1 = &arms[1];
            // arm1 body now should be:
            //   Call, DefVar, ReleaseAuto, Call, TailCall
            assert!(
                matches!(arm1.body[2], IrInst::ReleaseAuto(20)),
                "release should be inserted right after DefVar in arm body, got {:?}",
                arm1.body
            );
        } else {
            panic!("expected CondBranch");
        }
    }

    /// Captured into closure: should NOT release.
    #[test]
    fn captured_binding_skipped() {
        let mut func = IrFunction::new("f".to_string());
        func.body
            .push(IrInst::Call(0, "taida_str_repeat".to_string(), vec![]));
        func.body.push(IrInst::DefVar("s".to_string(), 0));
        func.body.push(IrInst::MakeClosure(
            1,
            "lambda_0".to_string(),
            vec!["s".to_string()],
        ));
        func.body.push(IrInst::Return(1));
        run(&mut func);
        assert_eq!(
            release_count(&func.body),
            0,
            "captured binding must not be released"
        );
    }

    /// A name that the function-end Release list covers keeps that single
    /// release: no ReleaseAuto may be added for it anywhere else.
    #[test]
    fn function_end_release_prevents_double_release() {
        let mut func = IrFunction::new("foo".to_string());
        // Outer body: PackNew; DefVar("p"); CondBranch; UseVar(t, "p"); Release(t); Return
        func.body.push(IrInst::PackNew(1, 1));
        func.body.push(IrInst::DefVar("p".to_string(), 1));
        let arm1 = CondArm {
            condition: Some(99),
            body: vec![IrInst::ConstInt(10, 1)],
            result: 10,
        };
        let arm2 = CondArm {
            condition: None,
            body: vec![IrInst::ConstInt(21, 0)],
            result: 21,
        };
        func.body.push(IrInst::CondBranch(0, vec![arm1, arm2]));
        func.body.push(IrInst::UseVar(30, "p".to_string()));
        func.body.push(IrInst::Release(30));
        func.body.push(IrInst::ConstInt(31, 0));
        func.body.push(IrInst::Return(31));

        run(&mut func);
        assert_eq!(release_count(&func.body), 0);
        assert_eq!(function_end_release_count(&func.body), 1);
    }

    /// A function-end Release of a name bound only inside an arm reads a
    /// value that does not dominate the function end (and may be the arm's
    /// result): it is dropped, and the arm-local dead binding is released
    /// inside its arm instead.
    #[test]
    fn arm_local_function_end_release_is_dropped() {
        let mut func = IrFunction::new("foo".to_string());
        let arm1 = CondArm {
            condition: Some(99),
            body: vec![IrInst::ConstInt(10, 1)],
            result: 10,
        };
        let arm2 = CondArm {
            condition: None,
            body: vec![
                IrInst::PackNew(20, 1),
                IrInst::DefVar("p".to_string(), 20),
                IrInst::ConstInt(21, 0),
            ],
            result: 21,
        };
        func.body.push(IrInst::CondBranch(0, vec![arm1, arm2]));
        func.body.push(IrInst::UseVar(30, "p".to_string()));
        func.body.push(IrInst::Release(30));
        func.body.push(IrInst::ConstInt(31, 0));
        func.body.push(IrInst::Return(31));

        run(&mut func);
        assert_eq!(function_end_release_count(&func.body), 0);
        assert!(
            !func
                .body
                .iter()
                .any(|inst| matches!(inst, IrInst::UseVar(_, name) if name == "p")),
            "the function end must not read the arm-local name"
        );
        assert_eq!(release_count(&func.body), 1);
    }

    /// The value an arm yields through a tail binding is the arm's result.
    #[test]
    fn arm_tail_binding_is_not_released() {
        let mut func = IrFunction::new("describe".to_string());
        let arm = CondArm {
            condition: None,
            body: vec![
                IrInst::ConstStr(20, "pos-".to_string()),
                IrInst::ConstStr(21, "5".to_string()),
                IrInst::Call(22, "taida_str_concat".to_string(), vec![20, 21]),
                IrInst::DefVar("label".to_string(), 22),
            ],
            result: 22,
        };
        func.body.push(IrInst::CondBranch(0, vec![arm]));
        func.body.push(IrInst::Return(0));
        run(&mut func);
        assert_eq!(release_count(&func.body), 0);
    }

    /// `unused <= xs` only aliases `xs`: releasing it would free `xs`.
    #[test]
    fn alias_binding_is_not_released() {
        let mut func = IrFunction::new("_taida_main".to_string());
        func.body
            .push(IrInst::Call(0, "taida_list_new".to_string(), vec![]));
        func.body.push(IrInst::DefVar("xs".to_string(), 0));
        func.body.push(IrInst::UseVar(1, "xs".to_string()));
        func.body.push(IrInst::DefVar("unused".to_string(), 1));
        func.body.push(IrInst::UseVar(2, "xs".to_string()));
        func.body.push(IrInst::ConstInt(3, 5));
        func.body.push(IrInst::Call(
            4,
            "taida_io_stdout_with_tag".to_string(),
            vec![2, 3],
        ));
        func.body.push(IrInst::Return(3));
        run(&mut func);
        assert_eq!(release_count(&func.body), 0);
    }

    /// Values restored from an environment pack (closure captures,
    /// error-ceiling bodies, type-method fields) are borrowed.
    #[test]
    fn env_pack_values_are_not_released() {
        let mut func =
            IrFunction::new_with_params("_taida_try_0".to_string(), vec!["__env".to_string()]);
        func.body.push(IrInst::PackGet(1, 0, 0));
        func.body.push(IrInst::DefVar("xs".to_string(), 1));
        func.body.push(IrInst::PackGet(2, 0, 1));
        func.body.push(IrInst::DefVar("n".to_string(), 2));
        func.body.push(IrInst::UseVar(3, "n".to_string()));
        func.body.push(IrInst::Return(3));
        run(&mut func);
        assert_eq!(release_count(&func.body), 0);
    }

    /// A user call's result is released only when the callee always hands
    /// back a value of its own (`mk`), never when it may return an argument
    /// (`id`).
    #[test]
    fn call_results_are_released_only_when_the_callee_returns_fresh_values() {
        let mut mk = IrFunction::new("mk".to_string());
        mk.body
            .push(IrInst::Call(0, "taida_str_repeat".to_string(), vec![]));
        mk.body.push(IrInst::DefVar("s".to_string(), 0));
        mk.body.push(IrInst::UseVar(1, "s".to_string()));
        mk.body.push(IrInst::Return(1));

        let mut id = IrFunction::new_with_params("id".to_string(), vec!["x".to_string()]);
        id.body.push(IrInst::UseVar(1, "x".to_string()));
        id.body.push(IrInst::Return(1));

        let mut main = IrFunction::new("_taida_main".to_string());
        main.body
            .push(IrInst::CallUser(0, "mk".to_string(), vec![]));
        main.body.push(IrInst::DefVar("a".to_string(), 0));
        main.body
            .push(IrInst::Call(1, "taida_list_new".to_string(), vec![]));
        main.body.push(IrInst::DefVar("xs".to_string(), 1));
        main.body.push(IrInst::UseVar(2, "xs".to_string()));
        main.body
            .push(IrInst::CallUser(3, "id".to_string(), vec![2]));
        main.body.push(IrInst::DefVar("b".to_string(), 3));
        main.body.push(IrInst::UseVar(4, "xs".to_string()));
        main.body.push(IrInst::Return(4));

        let mut module = IrModule::new();
        module.functions = vec![mk, id, main];
        insert_release_for_dead_bindings(&mut module);
        let main = &module.functions[2];
        assert!(
            main.body
                .iter()
                .any(|i| matches!(i, IrInst::ReleaseAuto(0)))
        );
        assert!(
            !main
                .body
                .iter()
                .any(|i| matches!(i, IrInst::ReleaseAuto(3)))
        );
    }

    /// A value captured by a returned closure must survive the function:
    /// its function-end Release is dropped.
    #[test]
    fn returned_closure_capture_keeps_its_value() {
        let mut func = IrFunction::new("makeGetter".to_string());
        func.body
            .push(IrInst::Call(0, "taida_list_new".to_string(), vec![]));
        func.body.push(IrInst::DefVar("xs".to_string(), 0));
        func.body.push(IrInst::MakeClosure(
            1,
            "lambda_0".to_string(),
            vec!["xs".to_string()],
        ));
        func.body.push(IrInst::DefVar("get".to_string(), 1));
        func.body.push(IrInst::UseVar(2, "get".to_string()));
        func.body.push(IrInst::UseVar(3, "xs".to_string()));
        func.body.push(IrInst::Release(3));
        func.body.push(IrInst::Return(2));
        run(&mut func);
        assert_eq!(function_end_release_count(&func.body), 0);
    }

    fn arm_body(func: &IrFunction, arm: usize) -> &Vec<IrInst> {
        func.body
            .iter()
            .find_map(|inst| match inst {
                IrInst::CondBranch(_, arms) => Some(&arms[arm].body),
                _ => None,
            })
            .expect("expected CondBranch")
    }

    /// `loop i acc = | i == 0 |> acc | _ |> s <= "a" + "b"; loop(i - 1, acc + s.length())`
    fn string_loop() -> IrFunction {
        let mut func =
            IrFunction::new_with_params("loop".to_string(), vec!["i".into(), "acc".into()]);
        func.body.push(IrInst::UseVar(2, "i".into()));
        func.body.push(IrInst::ConstInt(3, 0));
        func.body
            .push(IrInst::Call(4, "taida_int_eq".into(), vec![2, 3]));
        let base = CondArm {
            condition: Some(4),
            body: vec![IrInst::UseVar(10, "acc".into())],
            result: 10,
        };
        let step = CondArm {
            condition: None,
            body: vec![
                IrInst::ConstStr(20, "a".into()),
                IrInst::ConstStr(21, "b".into()),
                IrInst::Call(22, "taida_str_concat".into(), vec![20, 21]),
                IrInst::DefVar("s".into(), 22),
                IrInst::UseVar(23, "i".into()),
                IrInst::ConstInt(24, 1),
                IrInst::Call(25, "taida_int_sub".into(), vec![23, 24]),
                IrInst::UseVar(26, "acc".into()),
                IrInst::UseVar(27, "s".into()),
                IrInst::Call(28, "taida_polymorphic_length".into(), vec![27]),
                IrInst::Call(29, "taida_int_add".into(), vec![26, 28]),
                IrInst::TailCall(vec![25, 29]),
                IrInst::ConstInt(30, 0),
            ],
            result: 30,
        };
        func.body.push(IrInst::CondBranch(5, vec![base, step]));
        func.body.push(IrInst::Return(5));
        func.next_var = 31;
        func
    }

    /// A value used by the iteration is released before the back-edge.
    #[test]
    fn iteration_values_are_released_before_the_tail_call() {
        let mut func = string_loop();
        run(&mut func);
        let step = arm_body(&func, 1);
        let tail = step
            .iter()
            .position(|i| matches!(i, IrInst::TailCall(_)))
            .unwrap();
        assert!(
            step[..tail]
                .iter()
                .any(|i| matches!(i, IrInst::ReleaseAuto(22))),
            "{step:?}"
        );
    }

    /// A fresh value passed as the next parameter value becomes loop-owned,
    /// and the previous one is released when it is replaced or when the
    /// loop ends without returning it.
    #[test]
    fn loop_parameters_track_ownership() {
        let mut func =
            IrFunction::new_with_params("loop".to_string(), vec!["i".into(), "acc".into()]);
        func.body.push(IrInst::UseVar(2, "i".into()));
        func.body.push(IrInst::ConstInt(3, 0));
        func.body
            .push(IrInst::Call(4, "taida_int_eq".into(), vec![2, 3]));
        let base = CondArm {
            condition: Some(4),
            body: vec![
                IrInst::UseVar(10, "acc".into()),
                IrInst::Call(11, "taida_polymorphic_length".into(), vec![10]),
            ],
            result: 11,
        };
        let step = CondArm {
            condition: None,
            body: vec![
                IrInst::UseVar(20, "i".into()),
                IrInst::ConstInt(21, 1),
                IrInst::Call(22, "taida_int_sub".into(), vec![20, 21]),
                IrInst::UseVar(23, "acc".into()),
                IrInst::ConstStr(24, "x".into()),
                IrInst::Call(25, "taida_str_concat".into(), vec![23, 24]),
                IrInst::TailCall(vec![22, 25]),
                IrInst::ConstInt(26, 0),
            ],
            result: 26,
        };
        func.body.push(IrInst::CondBranch(5, vec![base, step]));
        func.body.push(IrInst::Return(5));
        func.next_var = 27;
        run(&mut func);

        let step = arm_body(&func, 1);
        let tail = step
            .iter()
            .position(|i| matches!(i, IrInst::TailCall(_)))
            .unwrap();
        let before_tail = &step[..tail];
        assert!(
            before_tail
                .iter()
                .any(|i| matches!(i, IrInst::ReleaseParamIfOwned(1))),
            "{step:?}"
        );
        assert!(
            before_tail
                .iter()
                .any(|i| matches!(i, IrInst::SetParamOwned(1, true))),
            "{step:?}"
        );
        assert!(
            !before_tail
                .iter()
                .any(|i| matches!(i, IrInst::ReleaseAuto(25))),
            "the next parameter value must not be released: {step:?}"
        );
        let ret = func
            .body
            .iter()
            .position(|i| matches!(i, IrInst::Return(_)))
            .unwrap();
        assert!(matches!(func.body[ret - 1], IrInst::ReleaseParamIfOwned(1)));
    }

    /// A loop that returns its parameter hands it to the caller.
    #[test]
    fn returned_loop_parameter_is_not_released() {
        let mut func = string_loop();
        run(&mut func);
        let ret = func
            .body
            .iter()
            .position(|i| matches!(i, IrInst::Return(_)))
            .unwrap();
        assert!(
            !matches!(func.body[ret - 1], IrInst::ReleaseParamIfOwned(_)),
            "{:?}",
            func.body
        );
    }

    /// `fill(m.set(k, v), ...)`: the update consumes the loop's map.
    #[test]
    fn map_update_of_a_loop_parameter_is_consuming() {
        let mut func =
            IrFunction::new_with_params("fill".to_string(), vec!["m".into(), "i".into()]);
        func.body.push(IrInst::UseVar(2, "i".into()));
        func.body.push(IrInst::ConstInt(3, 0));
        func.body
            .push(IrInst::Call(4, "taida_int_eq".into(), vec![2, 3]));
        let base = CondArm {
            condition: Some(4),
            body: vec![IrInst::UseVar(10, "m".into())],
            result: 10,
        };
        let step = CondArm {
            condition: None,
            body: vec![
                IrInst::UseVar(20, "m".into()),
                IrInst::UseVar(21, "i".into()),
                IrInst::Call(22, "taida_value_hash".into(), vec![21]),
                IrInst::Call(23, "taida_hashmap_set_immut".into(), vec![20, 22, 21, 21]),
                IrInst::UseVar(24, "i".into()),
                IrInst::ConstInt(25, 1),
                IrInst::Call(26, "taida_int_sub".into(), vec![24, 25]),
                IrInst::TailCall(vec![23, 26]),
                IrInst::ConstInt(27, 0),
            ],
            result: 27,
        };
        func.body.push(IrInst::CondBranch(5, vec![base, step]));
        func.body.push(IrInst::Return(5));
        func.next_var = 28;
        run(&mut func);

        let step = arm_body(&func, 1);
        assert!(
            step.iter().any(|i| matches!(i, IrInst::Call(23, name, args)
                if name == "taida_hashmap_set_consume" && args.len() == 5)),
            "{step:?}"
        );
        assert!(step.iter().any(|i| matches!(i, IrInst::ParamOwned(_, 0))));
        assert!(
            step.iter()
                .any(|i| matches!(i, IrInst::SetParamOwned(0, true)))
        );
        assert!(
            !step
                .iter()
                .any(|i| matches!(i, IrInst::ReleaseParamIfOwned(0))),
            "the consuming update already took the map over: {step:?}"
        );
    }

    /// Reading the old map after the update keeps the copying update.
    #[test]
    fn map_update_stays_copying_when_the_old_map_is_read_afterwards() {
        let mut func =
            IrFunction::new_with_params("fill".to_string(), vec!["m".into(), "i".into()]);
        func.body.push(IrInst::UseVar(2, "i".into()));
        func.body.push(IrInst::ConstInt(3, 0));
        func.body
            .push(IrInst::Call(4, "taida_int_eq".into(), vec![2, 3]));
        let base = CondArm {
            condition: Some(4),
            body: vec![IrInst::UseVar(10, "m".into())],
            result: 10,
        };
        let step = CondArm {
            condition: None,
            body: vec![
                IrInst::UseVar(20, "m".into()),
                IrInst::UseVar(21, "i".into()),
                IrInst::Call(22, "taida_value_hash".into(), vec![21]),
                IrInst::Call(23, "taida_hashmap_set_immut".into(), vec![20, 22, 21, 21]),
                IrInst::UseVar(24, "m".into()),
                IrInst::Call(25, "taida_collection_size".into(), vec![24]),
                IrInst::TailCall(vec![23, 25]),
                IrInst::ConstInt(27, 0),
            ],
            result: 27,
        };
        func.body.push(IrInst::CondBranch(5, vec![base, step]));
        func.body.push(IrInst::Return(5));
        func.next_var = 28;
        run(&mut func);
        let step = arm_body(&func, 1);
        assert!(
            !step.iter().any(|i| matches!(i, IrInst::Call(_, name, _)
                if name == "taida_hashmap_set_consume")),
            "{step:?}"
        );
    }

    /// An item appended to a list is owned by the new list afterwards,
    /// even when the call reads it through an alias of its binding.
    #[test]
    fn appended_item_is_not_released() {
        let mut func = IrFunction::new_with_params("add".to_string(), vec!["list".into()]);
        func.body.push(IrInst::PackNew(1, 1));
        func.body.push(IrInst::DefVar("item".into(), 1));
        func.body.push(IrInst::UseVar(2, "list".into()));
        func.body.push(IrInst::UseVar(3, "item".into()));
        func.body.push(IrInst::ConstInt(4, 4));
        func.body
            .push(IrInst::Call(5, "taida_list_append_k".into(), vec![2, 3, 4]));
        func.body.push(IrInst::Return(5));
        func.next_var = 6;
        run(&mut func);
        assert_eq!(release_count(&func.body), 0, "{:?}", func.body);
    }

    /// A value handed to a runtime call that may keep it is never released.
    #[test]
    fn value_kept_by_an_unknown_call_is_not_released() {
        let mut func = IrFunction::new_with_params("f".to_string(), vec!["x".into()]);
        func.body
            .push(IrInst::Call(1, "taida_str_repeat".into(), vec![]));
        func.body.push(IrInst::DefVar("s".into(), 1));
        func.body.push(IrInst::UseVar(2, "s".into()));
        func.body
            .push(IrInst::Call(3, "taida_async_task_new".into(), vec![2]));
        func.body.push(IrInst::ConstInt(4, 0));
        func.body.push(IrInst::Return(4));
        func.next_var = 5;
        run(&mut func);
        assert_eq!(release_count(&func.body), 0, "{:?}", func.body);
    }

    /// Stored into Pack via PackSet: should NOT release.
    #[test]
    fn pack_set_value_skipped() {
        let mut func = IrFunction::new("f".to_string());
        func.body
            .push(IrInst::Call(0, "taida_str_repeat".to_string(), vec![]));
        func.body.push(IrInst::DefVar("s".to_string(), 0));
        func.body.push(IrInst::PackNew(1, 1));
        func.body.push(IrInst::PackSet(1, 0, 0));
        func.body.push(IrInst::Return(1));
        run(&mut func);
        assert_eq!(
            release_count(&func.body),
            0,
            "value stored into Pack must not be released"
        );
    }
}
