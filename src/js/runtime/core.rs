//! JS runtime: core helpers, types, arithmetic, I/O, regex, stream.
//!
//! Split out from monolithic `src/js/runtime.rs` as part of C12-9
//! (FB-21 mechanical file split). This chunk covers helpers / Lax /
//! Result / BuchiPack / throw / async / Regex / stream / stdout /
//! stderr / stdin / trampoline / format / toString, plus the
//! HashMap / Set / equals / typeof / spread helpers. Original
//! source line range: 4..2003.
//!
//! See `src/js/runtime/mod.rs::RUNTIME_JS` for the assembled constant
//! and the file-split boundary table at
//! `.dev/taida-logs/docs/design/file_boundaries.md`.

pub(super) const CORE_JS: &str = r#"
// taida-runtime.js — Taida Lang JavaScript Runtime

// Save native constructors before Taida's functions shadow them
const __NativeError = globalThis.Error;
const __NativeMap = globalThis.Map;
const __NativeSet = globalThis.Set;

function __taida_debug(...args) {
  for (const arg of args) {
    if (__taida_isBytes(arg)) {
      console.log(__taida_bytes_to_string(arg));
    } else
    if (arg && arg.__type) {
      console.log(arg.__type + '(' + JSON.stringify(arg) + ')');
    } else if (Array.isArray(arg)) {
      console.log('@[' + arg.map(x => typeof x === 'string' ? '"' + x + '"' : String(x)).join(', ') + ']');
    } else {
      console.log(typeof arg === 'boolean' ? (arg ? 'true' : 'false') : String(arg));
    }
  }
}

function __taida_ensureNotNull(value, defaultValue) {
  return (value === null || value === undefined) ? defaultValue : value;
}

function __taida_escape_str(s) {
  return s.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\n/g, '\\n').replace(/\r/g, '\\r').replace(/\t/g, '\\t').replace(/\0/g, '\\0');
}

function __taida_solidify(value) {
  if (value && typeof value === 'object' && typeof value.solidify === 'function') {
    return value.solidify();
  }
  return value;
}

function __taida_defaultValue(typeName) {
  switch (typeName) {
    case 'Int': return 0;
    case 'Float': return 0.0;
    case 'Str': return '';
    case 'Bytes': return new Uint8Array(0);
    case 'Bool': return false;
    default: return Object.freeze({});
  }
}

function __taida_isIntNumber(v) {
  return typeof v === 'number' && Number.isFinite(v) && Number.isInteger(v);
}

function __taida_toI64BigInt(v) {
  if (typeof v === 'bigint') return BigInt.asIntN(64, v);
  if (__taida_isIntNumber(v)) return BigInt.asIntN(64, BigInt(v));
  if (typeof v === 'boolean') return v ? 1n : 0n;
  if (typeof v === 'string' && /^-?\d+$/.test(v)) {
    try { return BigInt.asIntN(64, BigInt(v)); } catch (_) { return 0n; }
  }
  return 0n;
}

function __taida_fromI64BigInt(v) {
  return Number(BigInt.asIntN(64, v));
}

function __taida_add(a, b) {
  if (__taida_isIntNumber(a) && __taida_isIntNumber(b)) {
    return __taida_fromI64BigInt(__taida_toI64BigInt(a) + __taida_toI64BigInt(b));
  }
  return a + b;
}

function __taida_sub(a, b) {
  if (__taida_isIntNumber(a) && __taida_isIntNumber(b)) {
    return __taida_fromI64BigInt(__taida_toI64BigInt(a) - __taida_toI64BigInt(b));
  }
  return a - b;
}

function __taida_mul(a, b) {
  if (__taida_isIntNumber(a) && __taida_isIntNumber(b)) {
    return __taida_fromI64BigInt(__taida_toI64BigInt(a) * __taida_toI64BigInt(b));
  }
  return a * b;
}

function __taida_isBytes(v) {
  return v instanceof Uint8Array;
}

function __taida_bytes_to_string(bytes) {
  return 'Bytes[@[' + Array.from(bytes).join(', ') + ']]';
}

function __taida_to_bytes_payload(data) {
  if (data instanceof Uint8Array) return Buffer.from(data);
  if (typeof Buffer !== 'undefined' && Buffer.isBuffer(data)) return data;
  if (typeof data === 'string') return Buffer.from(data, 'utf-8');
  if (Array.isArray(data)) {
    const ok = data.every(v => __taida_isIntNumber(v) && v >= 0 && v <= 255);
    if (ok) return Buffer.from(data);
  }
  return Buffer.from(String(data ?? ''), 'utf-8');
}

function __taida_lax_from_bytes(bytes, hasValue) {
  const val = bytes instanceof Uint8Array ? new Uint8Array(bytes) : new Uint8Array(0);
  return Object.freeze({
    __type: 'Lax',
    __value: val,
    __default: new Uint8Array(0),
    hasValue: __taida_hasValue(!!hasValue),
    isEmpty() { return !hasValue; },
    getOrDefault(def) { return hasValue ? val : def; },
    map(fn) { return hasValue ? Lax(fn(val)) : this; },
    flatMap(fn) {
      if (!hasValue) return this;
      const result = fn(val);
      if (result && result.__type === 'Lax') return result;
      return Lax(result);
    },
    unmold() { return hasValue ? val : new Uint8Array(0); },
    toString() {
      return hasValue ? 'Lax(' + __taida_bytes_to_string(val) + ')' : 'Lax(default: ' + __taida_bytes_to_string(new Uint8Array(0)) + ')';
    },
  });
}

function __taida_buchiPack(fields) {
  return Object.freeze(fields);
}

function __taida_list(items) {
  return Object.freeze([...items]);
}

class __TaidaError extends globalThis.Error {
  constructor(type, message, fields) {
    super(message);
    this.type = type;
    this.fields = fields || {};
  }
}

// Standalone throw function (no Object.prototype pollution)
function __taida_throw(obj) {
  throw obj instanceof globalThis.Error ? obj : new __TaidaError(obj.type || 'Error', obj.message || '', obj);
}

// RCB-101: Inheritance parent map for error type filtering in |==
// Use globalThis so the registry is shared across ESM modules (each .mjs
// embeds its own runtime copy, but all modules must see every parent
// registration so that cross-module error subtype checks work).
if (!globalThis.__taida_type_parents) globalThis.__taida_type_parents = {};
const __taida_type_parents = globalThis.__taida_type_parents;

// RCB-101: Check if thrown_type IS-A handler_type (walks inheritance chain)
function __taida_is_error_subtype(thrown_type, handler_type) {
  if (handler_type === 'Error') return true;
  let current = thrown_type;
  for (let i = 0; i < 64; i++) {
    if (current === handler_type) return true;
    const parent = __taida_type_parents[current];
    if (!parent) break;
    current = parent;
  }
  return false;
}

// Taida Error base type (not JS Error constructor)
// Error = @(type: Str, message: Str)
function Error(fields) {
  const obj = {
    __type: 'Error',
    type: __taida_ensureNotNull(fields && fields.type, ''),
    message: __taida_ensureNotNull(fields && fields.message, ''),
  };
  return Object.freeze(obj);
}

// ── Async[T] — Promise-based (thenable) ─────────────────
// __TaidaAsync is a thenable: it implements .then() so that
// `await asyncObj` resolves to the inner value or rejects with the error.
// This enables ]=> to map to `await` in generated JS code.
class __TaidaAsync {
  constructor(value, error, status) {
    this.__type = 'Async';
    this.status = status || 'fulfilled';
    this.__value = value;
    this.__error = error;
  }
  // Thenable protocol — makes `await asyncObj` work
  then(resolve, reject) {
    if (this.status === 'rejected') {
      if (reject) reject(this.__error);
    } else {
      if (resolve) resolve(this.__value);
    }
  }
  unmold() {
    if (this.status === 'rejected') throw this.__error;
    return this.__value;
  }
  isPending() { return this.status === 'pending'; }
  isFulfilled() { return this.status === 'fulfilled'; }
  isRejected() { return this.status === 'rejected'; }
  map(fn) {
    if (this.status !== 'fulfilled') return this;
    return new __TaidaAsync(fn(this.__value), null, 'fulfilled');
  }
  getOrDefault(def) {
    if (this.status === 'fulfilled') return this.__value;
    return def;
  }
  toString() {
    if (this.status === 'fulfilled') {
      const v = this.__value;
      if (v && typeof v === 'object' && !Array.isArray(v) && Object.keys(v).length === 0) {
        return 'Async[fulfilled: @()]';
      }
      const valStr = (v && typeof v === 'object' && v.toString && !Array.isArray(v)) ? v.toString() : String(v);
      return 'Async[fulfilled: ' + valStr + ']';
    }
    if (this.status === 'rejected') return 'Async[rejected: ' + String(this.__error) + ']';
    return 'Async[pending]';
  }
}

function Async(value) {
  return new __TaidaAsync(value, null, 'fulfilled');
}

function AsyncReject(error) {
  return new __TaidaAsync(null, error, 'rejected');
}

// Build a pending Async from a Promise while preserving Async shape/toString.
function __taida_async_pending_from_promise(promise) {
  const asyncObj = new __TaidaAsync(undefined, null, 'pending');
  asyncObj.then = function(resolve, reject) {
    return promise.then(
      (value) => {
        asyncObj.status = 'fulfilled';
        asyncObj.__value = value;
        asyncObj.__error = null;
        if (resolve) return resolve(value);
        return value;
      },
      (error) => {
        asyncObj.status = 'rejected';
        asyncObj.__error = error;
        if (reject) return reject(error);
        throw error;
      }
    );
  };
  return asyncObj;
}

// ── Async aggregation — sync/async hybrid ───────────────
// When all inputs are __TaidaAsync (sync thenables), process synchronously.
// When true async Promises are present, use Promise.all/race.
function All(asyncList) {
  // Fast path: no true Promise in inputs.
  const hasPromise = asyncList.some(item =>
    (item && typeof item.then === 'function' && !(item instanceof __TaidaAsync))
    || (item instanceof __TaidaAsync && item.status === 'pending')
  );
  if (!hasPromise) {
    const values = [];
    for (const item of asyncList) {
      if (item instanceof __TaidaAsync) {
        if (item.status === 'rejected') throw item.__error;
        values.push(item.__value);
      } else {
        values.push(item);
      }
    }
    return new __TaidaAsync(Object.freeze(values), null, 'fulfilled');
  }
  // Async path: true Promises present — return a Promise
  return Promise.all(asyncList.map(item => Promise.resolve(item))).then(results => Object.freeze(results));
}

function Race(asyncList) {
  if (asyncList.length === 0) return new __TaidaAsync(Object.freeze({}));
  // Fast path: no true Promise in inputs.
  const hasPromise = asyncList.some(item =>
    (item && typeof item.then === 'function' && !(item instanceof __TaidaAsync))
    || (item instanceof __TaidaAsync && item.status === 'pending')
  );
  if (!hasPromise) {
    const first = asyncList[0];
    if (first instanceof __TaidaAsync) {
      if (first.status === 'rejected') throw first.__error;
      return first;
    }
    return new __TaidaAsync(first, null, 'fulfilled');
  }
  // Async path
  return Promise.race(asyncList);
}

function Timeout(asyncVal, ms) {
  // If sync __TaidaAsync, preserve Async shape for parity with Interpreter/Native.
  if (asyncVal instanceof __TaidaAsync) {
    if (asyncVal.status === 'rejected') throw asyncVal.__error;
    if (asyncVal.status === 'pending') {
      return Promise.race([
        Promise.resolve(asyncVal),
        new Promise((_, reject) => setTimeout(() => reject(new __TaidaError('TimeoutError', 'timeout', {})), ms))
      ]);
    }
    return asyncVal;
  }
  // Non-thenable value behaves as already-fulfilled Async.
  if (!asyncVal || typeof asyncVal.then !== 'function') {
    return new __TaidaAsync(asyncVal, null, 'fulfilled');
  }
  // Async path: race against timeout
  return Promise.race([
    asyncVal,
    new Promise((_, reject) => setTimeout(() => reject(new __TaidaError('TimeoutError', 'timeout', {})), ms))
  ]);
}

// ── JSON type — opaque (Molten Iron) ────────────────────
// JSON is opaque. No methods. Must be cast through schema: JSON[raw, Schema]()
class __TaidaJSON {
  constructor(v) { this.__type = 'JSON'; this.__value = v; }
}

// TypeDef registry for JSON schema resolution
const __taida_typeDefs = {};
function __taida_registerTypeDef(name, fieldDefs) {
  __taida_typeDefs[name] = fieldDefs;
}

// C16: Enum registry for JSON schema resolution.
// Maps enum_name -> variant_names_in_ordinal_order.
// Populated by __taida_registerEnumDef emitted alongside each `Enum => Name = :A :B` definition.
const __taida_enumDefs = {};
function __taida_registerEnumDef(name, variants) {
  __taida_enumDefs[name] = variants;
}

// C18-2: Construct a tagged Enum value that coerces to its ordinal Int for
// arithmetic / comparison but emits its declared variant-name Str when
// `JSON.stringify` is invoked (via `toJSON`). Mirrors the interpreter's
// `Value::EnumVal(enum_name, ordinal)` variant.
//
// The return is wrapped in a Number subclass equivalent so that:
//  - `a === b` (ordinal equality) still works with `Number.prototype`
//    primitive coercion under `==`;
//  - strict `===` against a primitive number requires `Number(a) === b`
//    — we handle this at the callsites that matter (binary ops below);
//  - `JSON.stringify` uses `toJSON` automatically (emits variant name);
//  - `typeof` reports `'object'`, not `'number'` — this is acceptable
//    because `typeof` on Enum values was never a contract, whereas the
//    ordinal equality / arithmetic was. If a future test relies on
//    `typeof`, we can revisit.
function __taida_enumVal(enumName, ordinal) {
  // Using a function-object wrapper — `new Number(ordinal)` is rejected
  // by eslint in many configs, and assigning enumerable own properties
  // keeps the object inspectable. `valueOf` drives primitive coercion
  // (arithmetic, comparisons, ==). `toJSON` drives JSON.stringify.
  const obj = Object.create(null);
  obj.__taida_enum_name = enumName;
  obj.__taida_enum_ordinal = ordinal;
  obj.valueOf = function () { return ordinal; };
  obj.toJSON = function () {
    const variants = __taida_enumDefs[enumName];
    if (variants && ordinal >= 0 && ordinal < variants.length) {
      return variants[ordinal];
    }
    return ordinal;
  };
  obj.toString = function () { return String(ordinal); };
  return obj;
}

// C18-2: Detect a tagged Enum value produced by `__taida_enumVal`.
function __taida_isEnumVal(v) {
  return v !== null && typeof v === 'object' && typeof v.__taida_enum_name === 'string' && typeof v.__taida_enum_ordinal === 'number';
}

// C18-2: Ordinal-coerce a value (enum wrapper or primitive number) for use
// in arithmetic / comparison contexts that require a primitive.
function __taida_enumOrdinal(v) {
  if (__taida_isEnumVal(v)) return v.__taida_enum_ordinal;
  return v;
}

// C18B-005 fix: strict ordinal extractor for the `Ordinal[]` mold.
// Mirrors the interpreter contract at
// `src/interpreter/mold_eval.rs:3373-3394`: rejects non-Enum inputs
// so `--no-check` cannot silently erase misuse. The companion Native
// check lives in `taida_ordinal_strict` in `core.c`.
function __taida_enumOrdinalStrict(v) {
  if (__taida_isEnumVal(v)) return v.__taida_enum_ordinal;
  let got;
  if (v === null || v === undefined) {
    got = 'Unit';
  } else if (typeof v === 'number') {
    got = Number.isInteger(v) ? 'Int' : 'Float';
  } else if (typeof v === 'string') {
    got = 'Str';
  } else if (typeof v === 'boolean') {
    got = 'Bool';
  } else if (Array.isArray(v)) {
    got = 'List';
  } else if (typeof v === 'object') {
    got = v.__type ? v.__type : 'Pack';
  } else {
    got = typeof v;
  }
  throw new __TaidaError(
    'RuntimeError',
    'Ordinal: argument must be an Enum value, got ' + got + '. '
      + 'Hint: pass an Enum variant such as `Ordinal[Color:Red()]()`.',
    {}
  );
}

// C16: Lax[Enum] shape identical to interpreter / native.
//   @(hasValue=false, __value=Int(0), __default=Int(0), __type="Lax")
// First-variant-is-default rule is encoded via Int(0). Delegates to
// `Lax(null, 0)` so the returned object carries the full Lax method set
// (`hasValue`, `getOrDefault`, `isEmpty`, `map`, `flatMap`, `unmold`,
// `toString`), preserving 3-backend parity for Lax-facing Taida code.
function __taida_laxEnumEmpty() {
  return Lax(null, 0);
}

function __taidaValueToJson(v) {
  if (v instanceof __TaidaJSON) return v.__value;
  if (Array.isArray(v)) return v.map(__taidaValueToJson);
  if (v && typeof v === 'object' && !Array.isArray(v)) {
    const result = {};
    for (const [k, val] of Object.entries(v)) result[k] = __taidaValueToJson(val);
    return result;
  }
  return v;
}

// Schema-based JSON casting: JSON[raw, Schema]() -> Lax
function JSON_mold(rawValue, schema) {
  // Parse raw data
  let jsonData;
  if (rawValue instanceof __TaidaJSON) {
    jsonData = rawValue.__value;
  } else if (typeof rawValue === 'string') {
    try { jsonData = JSON.parse(rawValue); }
    catch (e) {
      const defaultVal = __taida_defaultForSchema(schema);
      return Object.freeze({ __type: 'Lax', hasValue: __taida_hasValue(false), __value: defaultVal, __default: defaultVal, __error: 'JSON parse error: ' + e.message });
    }
  } else {
    jsonData = __taidaValueToJson(rawValue);
  }

  // Cast through schema
  const typedValue = __taida_castJson(jsonData, schema);
  const defaultVal = __taida_defaultForSchema(schema);
  return Object.freeze({ __type: 'Lax', hasValue: __taida_hasValue(true), __value: typedValue, __default: defaultVal });
}

// C16: Decide whether a field default for a missing/null Enum field should be
// a Lax[Enum] (silent coercion 禁止) or the regular schema default.
function __taida_fieldMissingDefault(fschema) {
  if (typeof fschema === 'string' && __taida_enumDefs[fschema]) {
    return __taida_laxEnumEmpty();
  }
  // Recurse into nested TypeDef so inner Enum fields also get Lax.
  if (typeof fschema === 'string' && __taida_typeDefs[fschema]) {
    const td = __taida_typeDefs[fschema];
    const result = { __type: fschema };
    for (const [fname, inner] of Object.entries(td)) {
      result[fname] = __taida_fieldMissingDefault(inner);
    }
    return Object.freeze(result);
  }
  // Inline BuchiPack
  if (fschema && typeof fschema === 'object' && !fschema.__list && !Array.isArray(fschema)) {
    const result = {};
    for (const [fname, inner] of Object.entries(fschema)) {
      result[fname] = __taida_fieldMissingDefault(inner);
    }
    return Object.freeze(result);
  }
  return __taida_defaultForSchema(fschema);
}

function __taida_castJson(json, schema) {
  if (typeof schema === 'string') {
    switch (schema) {
      case 'Int': return typeof json === 'number' ? Math.trunc(json) : (typeof json === 'string' ? (parseInt(json, 10) || 0) : 0);
      case 'Float': return typeof json === 'number' ? json : (typeof json === 'string' ? (parseFloat(json) || 0.0) : 0.0);
      case 'Str': return typeof json === 'string' ? json : (json === null || json === undefined ? '' : (typeof json === 'object' ? JSON.stringify(json) : String(json)));
      case 'Bool': return typeof json === 'boolean' ? json : false;
      default: {
        // C16: TypeDef wins over Enum when both exist (mirrors Interpreter).
        const td = __taida_typeDefs[schema];
        if (td) {
          if (typeof json !== 'object' || json === null || Array.isArray(json)) {
            return __taida_defaultForSchema(schema);
          }
          const result = { __type: schema };
          for (const [fname, fschema] of Object.entries(td)) {
            if (fname in json && json[fname] !== null && json[fname] !== undefined) {
              result[fname] = __taida_castJson(json[fname], fschema);
            } else {
              result[fname] = __taida_fieldMissingDefault(fschema);
            }
          }
          return Object.freeze(result);
        }
        // C16: Enum lookup — resolve variant name to ordinal, Lax on mismatch.
        const variants = __taida_enumDefs[schema];
        if (variants) {
          if (typeof json === 'string') {
            const idx = variants.indexOf(json);
            if (idx >= 0) return idx;
          }
          return __taida_laxEnumEmpty();
        }
        return __taida_defaultForSchema(schema);
      }
    }
  }
  if (schema && schema.__list) {
    if (!Array.isArray(json)) return Object.freeze([]);
    return Object.freeze(json.map(item => __taida_castJson(item, schema.__list)));
  }
  // Inline BuchiPack schema: { field1: schema1, field2: schema2 }
  if (schema && typeof schema === 'object' && !Array.isArray(schema)) {
    if (typeof json !== 'object' || json === null || Array.isArray(json)) {
      return __taida_defaultForSchema(schema);
    }
    const result = {};
    for (const [fname, fschema] of Object.entries(schema)) {
      if (fname in json && json[fname] !== null && json[fname] !== undefined) {
        result[fname] = __taida_castJson(json[fname], fschema);
      } else {
        result[fname] = __taida_fieldMissingDefault(fschema);
      }
    }
    return Object.freeze(result);
  }
  return '';
}

function __taida_defaultForSchema(schema) {
  if (typeof schema === 'string') {
    switch (schema) {
      case 'Int': return 0;
      case 'Float': return 0.0;
      case 'Str': return '';
      case 'Bool': return false;
      default: {
        const td = __taida_typeDefs[schema];
        if (td) {
          const result = { __type: schema };
          for (const [fname, fschema] of Object.entries(td)) {
            result[fname] = __taida_defaultForSchema(fschema);
          }
          return Object.freeze(result);
        }
        // C16: Enum default stays Int(0) (= first variant ordinal). Matches
        // Interpreter / Native. Lax is reserved for actual schema *mismatch*.
        if (__taida_enumDefs[schema]) return 0;
        return '';
      }
    }
  }
  if (schema && schema.__list) return Object.freeze([]);
  // Inline BuchiPack schema: { field1: schema1, field2: schema2 }
  if (schema && typeof schema === 'object' && !Array.isArray(schema)) {
    const result = {};
    for (const [fname, fschema] of Object.entries(schema)) {
      result[fname] = __taida_defaultForSchema(fschema);
    }
    return Object.freeze(result);
  }
  return '';
}

// ── Optional — ABOLISHED (v0.8.0) ────────────────────────
// Optional has been removed. Use Lax[value]() instead.
function Optional() { throw new __NativeError('Optional has been removed. Use Lax[value]() instead. Lax[T] provides the same safety with default value guarantees.'); }

// ── Some() / None() — ABOLISHED ──────────────────────────
function Some(_) { throw new __NativeError('Some() has been removed. Optional is abolished. Use Lax[value]() instead.'); }
function None() { throw new __NativeError('None() has been removed. Optional is abolished. Use Lax[value]() instead.'); }

// ── Result[T, P] (operation mold with predicate + throw field) ──
// Result[value, pred]() → P: :T => :Bool — predicate for success/failure
// Result[value, pred]() => r — predicate unevaluated (stored as __predicate)
// Result[value, pred]() ]=> r — predicate evaluated: true → value T, false → throw
// Result[value]() — backward compatible: no predicate (always success if no throw)
function __taida_result_create(value, throwVal, predicate) {
  const _value = value;
  const _throw = throwVal || null;
  const _pred = (typeof predicate === 'function') ? predicate : null;
  // Determine error state:
  // 1. If throw is explicitly set (not null), it's an error
  // 2. If predicate exists, evaluate P(value) — true = success, false = error
  // 3. No predicate + no throw = success (backward compatible)
  function _checkError() {
    if (_throw !== null && _throw !== undefined) {
      // If predicate exists, evaluate it even when throw is set
      if (_pred) {
        const predResult = _pred(_value);
        if (!predResult) return true; // predicate failed — error
        return false; // predicate passed even though throw was set — success
      }
      return true;
    }
    if (_pred) {
      const predResult = _pred(_value);
      return !predResult;
    }
    return false;
  }
  return Object.freeze({
    __type: 'Result',
    __value: _value,
    __predicate: _pred,
    throw: _throw,
    isSuccess() { return !_checkError(); },
    isError() { return _checkError(); },
    getOrDefault(def) { return _checkError() ? def : _value; },
    map(fn) {
      if (_checkError()) return this;
      return __taida_result_create(fn(_value), null, null);
    },
    flatMap(fn) {
      if (_checkError()) return this;
      const result = fn(_value);
      if (result && result.__type === 'Result') return result;
      return __taida_result_create(result, null, null);
    },
    mapError(fn) {
      if (!_checkError()) return this;
      const errMsg = _throw && _throw.message ? _throw.message : String(_throw);
      const newMsg = fn(errMsg);
      return __taida_result_create(null, { __type: 'ResultError', message: String(newMsg), type: 'ResultError' }, null);
    },
    getOrThrow() {
      if (!_checkError()) return _value;
      if (_throw && typeof _throw === 'object') {
        throw new __TaidaError(_throw.type || 'ResultError', _throw.message || String(_throw), {});
      }
      if (_throw) {
        throw new __TaidaError('ResultError', String(_throw), {});
      }
      // Predicate failed but no explicit throw — generate default error
      throw new __TaidaError('ResultError', 'Result predicate failed for value: ' + String(_value), {});
    },
    toString() {
      if (!_checkError()) return 'Result(' + String(_value) + ')';
      const errDisplay = _throw && _throw.message ? _throw.message : (_throw ? String(_throw) : 'predicate failed');
      return 'Result(throw <= ' + errDisplay + ')';
    },
    unmold() {
      if (_checkError()) {
        if (_throw && typeof _throw === 'object') {
          throw new __TaidaError(_throw.type || 'ResultError', _throw.message || String(_throw), {});
        }
        if (_throw) throw _throw;
        // Predicate failed but no explicit throw — generate default error
        throw new __TaidaError('ResultError', 'Result predicate failed for value: ' + String(_value), {});
      }
      return _value;
    },
  });
}

// ── Ok() / Err() — ABOLISHED ─────────────────────────────
function Ok(_) { throw new __NativeError('Ok() has been removed. Use Result[value]() instead.'); }
function Err(_) { throw new __NativeError('Err() has been removed. Use Result[value](throw <= error) instead.'); }

// Alias for MoldInst codegen — Result(value, predicate, opts)
function Result(...args) {
  // Result(value, pred, opts) — pred is function, opts may contain throw field
  // Result(value, opts) — backward compat: opts is object with throw field
  // Result(value) — success, no predicate
  let value = args.length > 0 ? args[0] : null;
  let pred = null;
  let throwVal = null;
  let argIdx = 1;
  // Check if second arg is a predicate (function) or options (object)
  if (argIdx < args.length && typeof args[argIdx] === 'function') {
    pred = args[argIdx];
    argIdx++;
  }
  // Check for options object with throw field
  if (argIdx < args.length && args[argIdx] && typeof args[argIdx] === 'object' && 'throw' in args[argIdx]) {
    throwVal = args[argIdx].throw;
    if (throwVal === null || throwVal === undefined) throwVal = null;
  }
  return __taida_result_create(value, throwVal, pred);
}

// ── Lax[T] ───────────────────────────────────────────────
function __taida_lax_default(value) {
  if (__taida_isBytes(value)) return new Uint8Array(0);
  if (typeof value === 'bigint') return 0;
  if (typeof value === 'number') return Number.isInteger(value) ? 0 : 0.0;
  if (typeof value === 'string') return '';
  if (typeof value === 'boolean') return false;
  if (Array.isArray(value)) return Object.freeze([]);
  if (value && typeof value === 'object') return Object.freeze({});
  return 0;
}

// Create a callable hasValue that also works as a Boolean-like property
// Allows both `x.hasValue` (field access) and `x.hasValue()` (method call)
function __taida_hasValue(val) {
  const fn = function() { return val; };
  fn.valueOf = function() { return val; };
  fn.toString = function() { return String(val); };
  return fn;
}

function Lax(value, typedDefault) {
  const _hasValue = value !== null && value !== undefined;
  const _default = _hasValue ? __taida_lax_default(value) : (typedDefault !== undefined ? typedDefault : 0);
  const _val = _hasValue ? value : _default;
  return Object.freeze({
    __type: 'Lax',
    __value: _val,
    __default: _default,
    hasValue: __taida_hasValue(_hasValue),
    isEmpty() { return !_hasValue; },
    getOrDefault(def) { return _hasValue ? _val : def; },
    map(fn) { return _hasValue ? Lax(fn(_val)) : this; },
    flatMap(fn) {
      if (!_hasValue) return this;
      const result = fn(_val);
      if (result && result.__type === 'Lax') return result;
      return Lax(result);
    },
    unmold() { return _hasValue ? _val : _default; },
    toString() {
      return _hasValue ? 'Lax(' + String(_val) + ')' : 'Lax(default: ' + String(_default) + ')';
    },
  });
}

function __taida_molten() {
  return Object.freeze({ __type: 'Molten' });
}

function __taida_stub(message) {
  if (typeof message !== 'string') {
    throw new __TaidaError(
      'TypeError',
      'Stub message must be a string literal/expression, got ' + String(message),
      {}
    );
  }
  return __taida_molten();
}

// TODO mold runtime factory. The `__type: 'TODO'` marker matches the source-
// level mold name and is used by `__taida_typeof`, `toString()`, and unmold
// dispatch to identify TODO values. This naming convention is shared by all
// mold types (Lax, Result, Gorillax, etc.).
function __taida_todo_mold(typeDefault, fields) {
  const f = fields && typeof fields === 'object' ? fields : {};
  const has = (name) => Object.prototype.hasOwnProperty.call(f, name);
  const unit = Object.freeze({});

  const id = has('id') ? f.id : unit;
  const task = has('task') ? f.task : unit;
  const sol = has('sol') ? f.sol : typeDefault;
  const unm = has('unm') ? f.unm : typeDefault;

  return Object.freeze({
    __type: 'TODO',
    id,
    task,
    sol,
    unm,
    __value: sol,
    __default: unm,
  });
}

// ── Gorillax/Cage Mold types ──
// Gorillax: like Lax but unmold failure = gorilla (program exit)
function Gorillax(value, error) {
  const _hasValue = value !== null && value !== undefined;
  const _error = error || null;
  return Object.freeze({
    __type: 'Gorillax',
    __value: _hasValue ? value : null,
    __error: _error,
    hasValue: __taida_hasValue(_hasValue),
    isEmpty() { return !_hasValue; },
    relax() {
      return Object.freeze({
        __type: 'RelaxedGorillax',
        __value: _hasValue ? value : null,
        __error: _error,
        hasValue: __taida_hasValue(_hasValue),
        isEmpty() { return !_hasValue; },
        toString() {
          return _hasValue ? 'RelaxedGorillax(' + String(value) + ')' : 'RelaxedGorillax(escaped)';
        },
      });
    },
    toString() {
      return _hasValue ? 'Gorillax(' + String(value) + ')' : 'Gorillax(><)';
    },
  });
}

// Cage: execute function in protected context, return Gorillax
function Cage_mold(cageValue, cageFn) {
  try {
    const result = cageFn(cageValue);
    return Gorillax(result, null);
  } catch (e) {
    return Gorillax(null, e);
  }
}

// ── Div/Mod Mold types (safe division returning Lax) ──
function Div_mold(a, b, opts) {
  if (opts === undefined) opts = {};
  const isFloat = opts.__floatHint || (typeof a === 'number' && (!Number.isInteger(a) || (typeof b === 'number' && !Number.isInteger(b))));
  const def = opts.default !== undefined ? opts.default : (isFloat ? 0.0 : 0);
  if (b === 0) return Lax(null, def);
  if (isFloat) return Lax(a / b);
  const result = Number.isInteger(a) && Number.isInteger(b) ? Math.trunc(a / b) : a / b;
  const lax = Lax(result);
  return lax;
}
function Mod_mold(a, b, opts) {
  if (opts === undefined) opts = {};
  const isFloat = opts.__floatHint || (typeof a === 'number' && (!Number.isInteger(a) || (typeof b === 'number' && !Number.isInteger(b))));
  const def = opts.default !== undefined ? opts.default : (isFloat ? 0.0 : 0);
  if (b === 0) return Lax(null, def);
  return Lax(a % b);
}

// ── Type Conversion Mold types (Str/Int/Float/Bool → Lax) ──
function Str_mold(value) {
  return Lax(String(value));
}
function __taida_parse_int_base(str, base) {
  if (!__taida_isIntNumber(base) || base < 2 || base > 36) return null;
  if (typeof str !== 'string' || str.length === 0) return null;
  let negative = false;
  let i = 0;
  if (str[0] === '-') {
    negative = true;
    i = 1;
  } else if (str[0] === '+') {
    i = 1;
  }
  if (i >= str.length) return null;
  const b = BigInt(base);
  let acc = 0n;
  for (; i < str.length; i++) {
    const ch = str[i].toLowerCase();
    let digit = -1;
    if (ch >= '0' && ch <= '9') digit = ch.charCodeAt(0) - 48;
    else if (ch >= 'a' && ch <= 'z') digit = ch.charCodeAt(0) - 87;
    if (digit < 0 || digit >= base) return null;
    acc = acc * b + BigInt(digit);
  }
  if (negative) acc = -acc;
  return __taida_fromI64BigInt(acc);
}
function Int_mold(value, base) {
  if (base !== undefined) {
    if (typeof value !== 'string') return Lax(null, 0);
    const parsed = __taida_parse_int_base(value, base);
    if (parsed === null) return Lax(null, 0);
    return Lax(parsed);
  }
  if (__taida_isIntNumber(value)) return Lax(value);
  if (typeof value === 'number') return Lax(Math.trunc(value));
  if (typeof value === 'bigint') return Lax(__taida_fromI64BigInt(value));
  if (typeof value === 'boolean') return Lax(value ? 1 : 0);
  if (typeof value === 'string') {
    if (!/^[+-]?\d+$/.test(value)) return Lax(null, 0);
    try {
      return Lax(__taida_fromI64BigInt(BigInt(value)));
    } catch (_) {
      return Lax(null, 0);
    }
  }
  return Lax(null, 0);
}
function Float_mold(value) {
  if (typeof value === 'number') return Lax(value);
  if (typeof value === 'bigint') return Lax(Number(value));
  if (typeof value === 'boolean') return Lax(value ? 1.0 : 0.0);
  if (typeof value === 'string') {
    const f = parseFloat(value);
    if (isNaN(f)) return Lax(null, 0.0);
    return Lax(f);
  }
  return Lax(null, 0.0);
}
function Bool_mold(value) {
  if (typeof value === 'boolean') return Lax(value);
  if (typeof value === 'bigint') return Lax(value !== 0n);
  if (typeof value === 'number') return Lax(value !== 0);
  if (typeof value === 'string') {
    if (value === 'true') return Lax(true);
    if (value === 'false') return Lax(false);
    return Lax(null, false);
  }
  return Lax(null, false);
}

function UInt8_mold(value) {
  if (__taida_isIntNumber(value) && value >= 0 && value <= 255) return Lax(value);
  if (typeof value === 'number' && Number.isFinite(value) && Number.isInteger(value) && value >= 0 && value <= 255) return Lax(value);
  if (typeof value === 'string' && /^-?\d+$/.test(value)) {
    const n = parseInt(value, 10);
    if (n >= 0 && n <= 255) return Lax(n);
  }
  return Lax(null, 0);
}

function Bytes_mold(value, opts) {
  const fill = opts && __taida_isIntNumber(opts.fill) ? opts.fill : 0;
  if (value instanceof Uint8Array) return __taida_lax_from_bytes(new Uint8Array(value), true);
  if (typeof value === 'string') {
    const buf = (typeof Buffer !== 'undefined')
      ? Buffer.from(value, 'utf-8')
      : new TextEncoder().encode(value);
    return __taida_lax_from_bytes(new Uint8Array(buf), true);
  }
  if (__taida_isIntNumber(value)) {
    if (value < 0 || fill < 0 || fill > 255) return __taida_lax_from_bytes(new Uint8Array(0), false);
    const arr = new Uint8Array(value);
    arr.fill(fill);
    return __taida_lax_from_bytes(arr, true);
  }
  if (Array.isArray(value)) {
    const ok = value.every(v => __taida_isIntNumber(v) && v >= 0 && v <= 255);
    if (!ok) return __taida_lax_from_bytes(new Uint8Array(0), false);
    return __taida_lax_from_bytes(new Uint8Array(value), true);
  }
  return __taida_lax_from_bytes(new Uint8Array(0), false);
}

function Char_mold(value) {
  if (__taida_isIntNumber(value)) {
    if (value < 0 || value > 0x10FFFF || (value >= 0xD800 && value <= 0xDFFF)) return Lax(null, '');
    try { return Lax(String.fromCodePoint(value)); } catch (_) { return Lax(null, ''); }
  }
  if (typeof value === 'string') {
    const chars = Array.from(value);
    if (chars.length === 1) return Lax(chars[0]);
  }
  return Lax(null, '');
}

function CodePoint_mold(value) {
  if (typeof value !== 'string') return Lax(null, 0);
  const chars = Array.from(value);
  if (chars.length !== 1) return Lax(null, 0);
  return Lax(chars[0].codePointAt(0));
}

function Utf8Encode_mold(value) {
  if (typeof value !== 'string') return __taida_lax_from_bytes(new Uint8Array(0), false);
  return Bytes_mold(value);
}

function Utf8Decode_mold(value) {
  if (!(value instanceof Uint8Array)) return Lax(null, '');
  try {
    const decoder = new TextDecoder('utf-8', { fatal: true });
    return Lax(decoder.decode(value));
  } catch (_) {
    return Lax(null, '');
  }
}

function BitAnd(a, b) {
  return __taida_fromI64BigInt(__taida_toI64BigInt(a) & __taida_toI64BigInt(b));
}
function BitOr(a, b) {
  return __taida_fromI64BigInt(__taida_toI64BigInt(a) | __taida_toI64BigInt(b));
}
function BitXor(a, b) {
  return __taida_fromI64BigInt(__taida_toI64BigInt(a) ^ __taida_toI64BigInt(b));
}
function BitNot(x) {
  return __taida_fromI64BigInt(~__taida_toI64BigInt(x));
}
function ShiftL(x, n) {
  if (!__taida_isIntNumber(n) || n < 0 || n > 63) return Lax(null, 0);
  return Lax(__taida_fromI64BigInt(__taida_toI64BigInt(x) << BigInt(n)));
}
function ShiftR(x, n) {
  if (!__taida_isIntNumber(n) || n < 0 || n > 63) return Lax(null, 0);
  return Lax(__taida_fromI64BigInt(__taida_toI64BigInt(x) >> BigInt(n)));
}
function ShiftRU(x, n) {
  if (!__taida_isIntNumber(n) || n < 0 || n > 63) return Lax(null, 0);
  const ux = BigInt.asUintN(64, __taida_toI64BigInt(x));
  return Lax(Number(ux >> BigInt(n)));
}
function ToRadix(value, base) {
  if (!__taida_isIntNumber(base) || base < 2 || base > 36) return Lax(null, '');
  return Lax(__taida_toI64BigInt(value).toString(base));
}

// ── Stream[T] — time-series mold type ────────────────────
// Stream holds source items + lazy transform chain.
// ]=> (unmold) collects all items, applying transforms.
function __taida_stream(items, transforms) {
  return Object.freeze({ __type: 'Stream', __items: Object.freeze([...items]), __transforms: Object.freeze([...transforms]), __status: 'completed',
    length_() { return this.__status === 'completed' ? this.__items.length : -1; },
    isEmpty() { return this.__items.length === 0 && this.__status === 'completed'; },
    toString() { return this.__status === 'active' ? 'Stream[active]' : 'Stream[completed: ' + this.__items.length + ' items]'; }
  });
}
function Stream_mold(value) { return __taida_stream([value], []); }
function StreamFrom(list) { return __taida_stream(list, []); }

// ── Map/Filter/Fold Mold types (return values directly, like interpreter) ──
// Stream input: append transform (lazy evaluation)
function Map(list, fn) {
  if (list && list.__type === 'Stream') return __taida_stream(list.__items, [...list.__transforms, { op: 'map', fn }]);
  return Object.freeze((list || []).map(item => fn(item)));
}
function Filter(list, fn) {
  if (list && list.__type === 'Stream') return __taida_stream(list.__items, [...list.__transforms, { op: 'filter', fn }]);
  return Object.freeze((list || []).filter(item => fn(item)));
}
function Fold(list, init, fn) {
  const items = (list && list.__type === 'Stream') ? __taida_stream_collect(list) : list;
  return (items || []).reduce((acc, item) => fn(acc, item), init);
}
function Reduce(list, init, fn) {
  const items = (list && list.__type === 'Stream') ? __taida_stream_collect(list) : list;
  return (items || []).reduce((acc, item) => fn(acc, item), init);
}
function Foldr(list, init, fn) {
  const items = (list && list.__type === 'Stream') ? __taida_stream_collect(list) : list;
  return (items || []).reduceRight((acc, item) => fn(acc, item), init);
}
function Take(list, n) {
  if (list && list.__type === 'Stream') return __taida_stream(list.__items, [...list.__transforms, { op: 'take', n }]);
  return Object.freeze(list.slice(0, n));
}
function TakeWhile(list, fn) {
  if (list && list.__type === 'Stream') return __taida_stream(list.__items, [...list.__transforms, { op: 'takeWhile', fn }]);
  const result = [];
  for (const item of list) { if (fn(item)) result.push(item); else break; }
  return Object.freeze(result);
}
function Drop(list, n) {
  const items = (list && list.__type === 'Stream') ? __taida_stream_collect(list) : list;
  return Object.freeze(items.slice(n));
}
function DropWhile(list, fn) {
  const items = (list && list.__type === 'Stream') ? __taida_stream_collect(list) : list;
  let dropping = true;
  const result = [];
  for (const item of items) {
    if (dropping && fn(item)) continue;
    dropping = false;
    result.push(item);
  }
  return Object.freeze(result);
}

// ── Stream collect helper (used by __taida_unmold) ───────
function __taida_stream_collect(stream) {
  let items = [...stream.__items];
  for (const t of stream.__transforms) {
    switch (t.op) {
      case 'map': items = items.map(item => t.fn(item)); break;
      case 'filter': items = items.filter(item => t.fn(item)); break;
      case 'take': items = items.slice(0, t.n); break;
      case 'takeWhile': {
        const r = [];
        for (const item of items) { if (t.fn(item)) r.push(item); else break; }
        items = r;
        break;
      }
    }
  }
  return Object.freeze(items);
}

// ── String Mold types ───────────────────────────────────
function Upper(str) { return typeof str === 'string' ? str.toUpperCase() : ''; }
function Lower(str) { return typeof str === 'string' ? str.toLowerCase() : ''; }
function Trim(str, opts) {
  if (typeof str !== 'string') return '';
  if (!opts) return str.trim();
  const doStart = opts.start !== false;
  const doEnd = opts.end !== false;
  if (doStart && doEnd) return str.trim();
  if (doStart && !doEnd) return str.trimStart();
  if (!doStart && doEnd) return str.trimEnd();
  return str;
}
function Split(str, delim) { return Object.freeze(typeof str === 'string' ? str.split(delim) : []); }
function Chars(str) { return Object.freeze(typeof str === 'string' ? Array.from(str) : []); }
function Replace(str, old, rep, opts) {
  if (typeof str !== 'string') return '';
  if (opts && opts.all) return str.split(old).join(rep);
  return str.replace(old, rep);
}
// B11-4c: Str method helpers (edge-case parity with Interpreter/Native)
// C12-6 (FB-5): Regex overload — when `target` / `sep` is a Regex pack
// (plain object with `__type === "Regex"`), compile a JS RegExp and
// delegate. Otherwise fall back to fixed-string semantics.
function __taida_is_regex(v) {
  return v !== null
    && typeof v === 'object'
    && v.__type === 'Regex'
    && typeof v.pattern === 'string';
}
// C12B-040: Rewrite `\x{HH..}` / `\u{HH..}` bracketed hex escapes to
// JS-native `\uHHHH` (or surrogate pair for supplementary planes).
// We intentionally do NOT enable the `u` flag on the RegExp, because
// `u` strict mode rejects harmless identity escapes like `\_` / `\/`
// that the Rust `regex` crate (Interpreter) and POSIX ERE (Native)
// both accept. The rewrite runs during both Regex(...) construction
// validation and at each compile_regex(...) site, guaranteeing that
// construction-time syntax matches first-use syntax (fixes C12B-040).
function __taida_rewrite_pattern(pat) {
  if (typeof pat !== 'string' || pat.length === 0) return pat;
  let out = '';
  let i = 0;
  while (i < pat.length) {
    const c = pat[i];
    if (c === '\\' && i + 1 < pat.length) {
      const n = pat[i + 1];
      if ((n === 'x' || n === 'u') && pat[i + 2] === '{') {
        // Bracketed hex escape: \x{HH..} or \u{HH..} (up to 8 hex digits).
        let j = i + 3;
        let digits = '';
        while (j < pat.length && pat[j] !== '}' && digits.length < 8) {
          const d = pat[j];
          if ((d >= '0' && d <= '9') || (d >= 'a' && d <= 'f') || (d >= 'A' && d <= 'F')) {
            digits += d;
            j++;
          } else {
            break;
          }
        }
        if (pat[j] === '}' && digits.length > 0) {
          const cp = parseInt(digits, 16);
          if (cp <= 0x10FFFF) {
            if (cp <= 0xFFFF) {
              // BMP: emit as \uHHHH (works without `u` flag in JS).
              out += '\\u' + digits.padStart(4, '0').toUpperCase().slice(-4);
            } else {
              // Supplementary: emit as UTF-16 surrogate pair so JS
              // matches the code point without the `u` flag.
              const v = cp - 0x10000;
              const hi = 0xD800 | (v >>> 10);
              const lo = 0xDC00 | (v & 0x3FF);
              out += '\\u' + hi.toString(16).toUpperCase().padStart(4, '0');
              out += '\\u' + lo.toString(16).toUpperCase().padStart(4, '0');
            }
            i = j + 1;
            continue;
          }
        }
        // Malformed bracketed escape — fall through and pass literally.
      }
      // Non-bracketed escapes (`\xHH`, `\uHHHH`, `\d`, `\_`, etc.)
      // are already valid in JS no-`u` mode — pass through verbatim.
      out += pat[i];
      out += pat[i + 1];
      i += 2;
      continue;
    }
    out += c;
    i++;
  }
  return out;
}
// C12B-036: FIFO-bounded compile cache for Regex objects. Without this,
// `s.replace(Regex("..."), "...")` in a tight loop recompiles the
// pattern on every iteration. The cache keys on the (pattern, flags,
// global) triple since the `global` parameter alters the final `g`
// flag. Capacity is intentionally small (64 distinct combos) to bound
// memory; when full we evict the oldest insertion. This is pure
// performance — V8/SpiderMonkey's RegExp grammar is unchanged.
const __TAIDA_REGEX_CACHE_CAPACITY = 64;
// NOTE: Use __NativeMap (aliased to globalThis.Map at the top of this
// prelude) because the local `Map` identifier is shadowed by the Taida
// `Map(list, fn)` mold function. `new Map()` without this alias would
// return a frozen empty array whose `.get` is Taida's patched
// Array.prototype.get returning Lax values — producing false cache HITs
// that corrupt every regex compile.
const __taida_regex_cache = new __NativeMap();
function __taida_regex_cache_get(pattern, flags, global) {
  const key = pattern + '\u0001' + flags + '\u0001' + (global ? '1' : '0');
  return { key: key, value: __taida_regex_cache.get(key) };
}
function __taida_regex_cache_put(key, re) {
  if (__taida_regex_cache.size >= __TAIDA_REGEX_CACHE_CAPACITY) {
    // Map iteration order is insertion order: drop the oldest.
    const oldest = __taida_regex_cache.keys().next().value;
    if (oldest !== undefined) __taida_regex_cache.delete(oldest);
  }
  __taida_regex_cache.set(key, re);
}
function __taida_compile_regex(rx, global) {
  // Strip `g` from user flags (`g` is controlled by the API: replaceAll
  // / replace / match / search); keep `i`, `m`, `s`. Unknown flag chars
  // were already rejected at Regex(...) construction time.
  const userFlags = typeof rx.flags === 'string' ? rx.flags.replace(/g/g, '') : '';
  const finalFlags = global ? userFlags + 'g' : userFlags;
  // C12B-036: cache lookup — skip both the rewrite pass and `new RegExp`
  // when the same (pattern, flags, global) triple has already been
  // compiled in this runtime.
  //
  // IMPORTANT: JS RegExp objects with the `g` flag carry mutable
  // `lastIndex` state across `.test()` / `.exec()` / stringy
  // `.match()`. We cannot safely return the same cached instance
  // because:
  //   (a) the caller's next `.match` / `.replace` / `.split` would
  //       resume from the previous `lastIndex`, silently skipping
  //       matches (observed in c12_6 parity tests), and
  //   (b) some runtimes (Node's ESM sealed RegExp subclass in strict
  //       mode) reject `lastIndex = 0` assignment on cached instances.
  // We therefore cache a pre-rewritten `(pattern, flags)` template and
  // build a fresh `RegExp` per call. The win is skipping the
  // `__taida_rewrite_pattern` scan on repeat hits — `new RegExp(src,
  // flags)` itself is unavoidable but still much cheaper than the
  // full rewrite+construct path.
  const cached = __taida_regex_cache_get(rx.pattern, finalFlags, global);
  if (cached.value !== undefined) {
    return new RegExp(cached.value.src, cached.value.flags);
  }
  // C12B-040: Rewrite bracketed hex escapes to native JS syntax instead
  // of turning on `/u`. See __taida_rewrite_pattern for rationale.
  const rewritten = __taida_rewrite_pattern(rx.pattern);
  const re = new RegExp(rewritten, finalFlags);
  __taida_regex_cache_put(cached.key, { src: rewritten, flags: finalFlags });
  return re;
}
// Escape literal `$` so that JS `String.prototype.replace` does not
// interpret `$&`, `$$`, `$1`, etc. as meta-syntax. Design lock §C12-6.
function __taida_escape_replacement(rep) {
  return typeof rep === 'string' ? rep.replace(/\$/g, '$$$$') : '';
}
function __taida_str_replace(s, target, rep) {
  if (typeof s !== 'string') return '';
  if (__taida_is_regex(target)) {
    // Regex overload: first match only, literal replacement.
    const re = __taida_compile_regex(target, false);
    return s.replace(re, __taida_escape_replacement(rep));
  }
  if (target === '') return s; // empty target → no-op (B11-4a)
  // Use indexOf+slice to avoid JS replacement meta-syntax ($&, $$, etc.)
  const idx = s.indexOf(target);
  if (idx === -1) return s;
  return s.slice(0, idx) + rep + s.slice(idx + target.length);
}
function __taida_str_replace_all(s, target, rep) {
  if (typeof s !== 'string') return '';
  if (__taida_is_regex(target)) {
    // Regex overload: global replace, literal replacement.
    const re = __taida_compile_regex(target, true);
    return s.replace(re, __taida_escape_replacement(rep));
  }
  if (target === '') return s; // empty target → no-op (B11-4a)
  return s.split(target).join(rep);
}
function __taida_str_split(s, sep) {
  if (typeof s !== 'string') return Object.freeze([]);
  if (__taida_is_regex(sep)) {
    const re = __taida_compile_regex(sep, false);
    return Object.freeze(s.split(re));
  }
  if (sep === '') return Object.freeze(s.length === 0 ? [] : Array.from(s));
  return Object.freeze(s.split(sep));
}
// C12-6a: Regex(pattern, flags?) prelude constructor. Validates at
// construction time so invalid patterns fail early (philosophy I).
function __taida_regex(pattern, flags) {
  const p = typeof pattern === 'string' ? pattern : '';
  const f = typeof flags === 'string' ? flags : '';
  // Validate flags (match the interpreter's `regex_eval::validate_flags`).
  // C12B-029: Use globalThis.Error rather than the local Taida `Error`
  // (which is shadowed by the prelude at the top of runtime/core.rs and
  // is frozen, so assigning __taida_error_type fails).
  for (const c of f) {
    if (c !== 'i' && c !== 'm' && c !== 's') {
      const err = new globalThis.Error(
        "Regex: unsupported flag '" + c + "'. Supported flags: i (case-insensitive), m (multiline), s (dotall)"
      );
      err.__taida_error_type = 'ValueError';
      throw err;
    }
  }
  // Validate the pattern by compiling once. Drop `g` because user
  // flags never control the global setting here (mirrors interpreter).
  // C12B-040: Apply the same bracketed-hex rewrite that
  // __taida_compile_regex uses at first-use, so construct-time and
  // use-time see the same grammar. Without this parity the user sees
  // a successful construct that later throws on .replace / .match /
  // .search with the same regex.
  try {
    const rewritten = __taida_rewrite_pattern(p);
    new RegExp(rewritten, f.replace(/g/g, ''));
  } catch (e) {
    const err = new globalThis.Error("Regex: invalid pattern '" + p + "' — " + e.message);
    err.__taida_error_type = 'ValueError';
    throw err;
  }
  return Object.freeze({ pattern: p, flags: f, __type: 'Regex' });
}
// C12-6c: str.match(Regex(...)) returns a :RegexMatch BuchiPack.
function __taida_str_match(s, rx) {
  const empty = Object.freeze({
    hasValue: false,
    full: '',
    groups: Object.freeze([]),
    start: -1,
    __type: 'RegexMatch',
  });
  if (typeof s !== 'string') return empty;
  if (!__taida_is_regex(rx)) {
    const err = new Error(
      'str.match(...) requires a Regex argument. Use Regex("pattern") to construct one.'
    );
    err.__taida_error_type = 'TypeError';
    throw err;
  }
  const re = __taida_compile_regex(rx, false);
  const m = s.match(re);
  if (!m) return empty;
  // Count chars (not UTF-16 code units) from string start to match
  // index so the returned `start` matches the interpreter / native
  // surface (char-based indices, see design lock §C12-6 and `indexOf`).
  const prefix = s.slice(0, m.index);
  const charStart = Array.from(prefix).length;
  const groups = [];
  for (let i = 1; i < m.length; i++) {
    groups.push(typeof m[i] === 'string' ? m[i] : '');
  }
  return Object.freeze({
    hasValue: true,
    full: m[0],
    groups: Object.freeze(groups),
    start: charStart,
    __type: 'RegexMatch',
  });
}
// C12-6c: str.search(Regex(...)) returns the char index of the first
// match, or -1 if none. The second arg must be a Regex — string
// search should use `.indexOf(...)`.
function __taida_str_search(s, rx) {
  if (typeof s !== 'string') return -1;
  if (!__taida_is_regex(rx)) {
    const err = new Error(
      'str.search(...) requires a Regex argument. Use Regex("pattern") to construct one.'
    );
    err.__taida_error_type = 'TypeError';
    throw err;
  }
  const re = __taida_compile_regex(rx, false);
  const m = s.match(re);
  if (!m) return -1;
  const prefix = s.slice(0, m.index);
  return Array.from(prefix).length;
}
function Slice(val, opts) {
  const start = (opts && __taida_isIntNumber(opts.start)) ? opts.start : 0;
  if (typeof val === 'string') {
    const end = (opts && __taida_isIntNumber(opts.end)) ? opts.end : val.length;
    return val.slice(start, end);
  }
  if (val instanceof Uint8Array) {
    const end = (opts && __taida_isIntNumber(opts.end)) ? opts.end : val.length;
    const s = Math.max(0, Math.min(val.length, start));
    const e = Math.max(0, Math.min(val.length, end));
    const from = Math.min(s, e);
    const to = Math.max(s, e);
    return val.slice(from, to);
  }
  return '';
}
function CharAt(str, idx) { return typeof str === 'string' && idx >= 0 && idx < str.length ? Lax(str[idx]) : Lax(null, ''); }
function Repeat(str, n) { return typeof str === 'string' ? str.repeat(Math.max(0, n)) : ''; }
function Reverse(val) {
  if (typeof val === 'string') return val.split('').reverse().join('');
  if (Array.isArray(val)) { const copy = [...val]; copy.reverse(); return Object.freeze(copy); }
  return val;
}
function Pad(str, len, opts) {
  if (typeof str !== 'string') return '';
  const side = (opts && opts.side) || 'start';
  const ch = (opts && opts.char) || ' ';
  if (side === 'start') return str.padStart(len, ch);
  if (side === 'end') return str.padEnd(len, ch);
  return str;
}

// ── Number Mold types ───────────────────────────────────
function ToFixed(num, digits) { return typeof num === 'number' ? num.toFixed(digits) : '0'; }
function Abs(num) { return typeof num === 'number' ? Math.abs(num) : 0; }
function Floor(num) { return typeof num === 'number' ? Math.floor(num) : 0; }
function Ceil(num) { return typeof num === 'number' ? Math.ceil(num) : 0; }
function Round(num) { return typeof num === 'number' ? Math.round(num) : 0; }
function Truncate(num) { return typeof num === 'number' ? Math.trunc(num) : 0; }
function Clamp(num, min, max) { return typeof num === 'number' ? Math.min(Math.max(num, min), max) : 0; }

// ── List Mold types (new operation molds) ───────────────
function Concat(list, other) {
  if (list instanceof Uint8Array && other instanceof Uint8Array) {
    const out = new Uint8Array(list.length + other.length);
    out.set(list, 0);
    out.set(other, list.length);
    return out;
  }
  return Object.freeze([...(list || []), ...(other || [])]);
}
function ByteSet(bytes, idx, value) {
  if (!(bytes instanceof Uint8Array) || !__taida_isIntNumber(idx) || !__taida_isIntNumber(value)) {
    return __taida_lax_from_bytes(new Uint8Array(0), false);
  }
  if (idx < 0 || idx >= bytes.length || value < 0 || value > 255) {
    return __taida_lax_from_bytes(new Uint8Array(0), false);
  }
  const out = new Uint8Array(bytes);
  out[idx] = value;
  return __taida_lax_from_bytes(out, true);
}
function BytesToList(bytes) {
  if (!(bytes instanceof Uint8Array)) return Object.freeze([]);
  return Object.freeze(Array.from(bytes, x => Number(x)));
}
function Append(list, val) { return Object.freeze([...(list || []), val]); }
function Prepend(list, val) { return Object.freeze([val, ...(list || [])]); }
function Join(list, sep) { return (list || []).join(sep); }
function Sum(list) { return (list || []).reduce((a, b) => a + b, 0); }
function Sort(list, opts) {
  const copy = [...(list || [])];
  if (opts && opts.by) {
    const fn = opts.by;
    copy.sort((a, b) => { const ka = fn(a), kb = fn(b); return ka < kb ? -1 : ka > kb ? 1 : 0; });
  } else {
    copy.sort((a, b) => a < b ? -1 : a > b ? 1 : 0);
  }
  if (opts && opts.reverse) copy.reverse();
  return Object.freeze(copy);
}
function Unique(list, opts) {
  const result = [];
  const arr = list || [];
  if (opts && opts.by) {
    const fn = opts.by;
    const seen = [];
    for (const item of arr) {
      const key = fn(item);
      if (!seen.some(k => __taida_equals(k, key))) { seen.push(key); result.push(item); }
    }
  } else {
    for (const item of arr) {
      if (!result.some(x => __taida_equals(x, item))) result.push(item);
    }
  }
  return Object.freeze(result);
}
function Flatten(list) {
  const result = [];
  for (const item of (list || [])) {
    if (Array.isArray(item)) result.push(...item); else result.push(item);
  }
  return Object.freeze(result);
}
function Find(list, fn) {
  for (let i = 0; i < (list || []).length; i++) {
    if (fn(list[i]) === true) return Lax(list[i]);
  }
  return Lax(null);
}
function FindIndex(list, fn) {
  for (let i = 0; i < (list || []).length; i++) {
    if (fn(list[i]) === true) return i;
  }
  return -1;
}
function Count(list, fn) {
  let c = 0;
  for (let i = 0; i < (list || []).length; i++) {
    if (fn(list[i]) === true) c++;
  }
  return c;
}
function Zip(list, other) {
  const a = list || [], b = other || [];
  const len = Math.min(a.length, b.length);
  const result = [];
  for (let i = 0; i < len; i++) result.push(Object.freeze({ first: a[i], second: b[i] }));
  return Object.freeze(result);
}
function Enumerate(list) {
  return Object.freeze((list || []).map((value, index) => Object.freeze({ index, value })));
}

// ── Trampoline for tail recursion (self + mutual) ────────
class __TaidaTailCall {
  constructor(fn, args) { this.fn = fn; this.args = args; }
}
function __taida_trampoline(fn) {
  return function(...args) {
    let result = fn(...args);
    while (result instanceof __TaidaTailCall) {
      result = result.fn(...result.args);
    }
    return result;
  };
}

function __taida_trampoline_async(fn) {
  return async function(...args) {
    let result = await fn(...args);
    while (result instanceof __TaidaTailCall) {
      result = await result.fn(...result.args);
    }
    return result;
  };
}

// ── stdout — Taida output function ───────────────────────
// C12-5 (FB-18): returns the total UTF-8 byte length of the rendered content
// (Int) so that `n <= stdout("hi")` binds `n = 2`. Mirrors the interpreter and
// native runtime. The trailing newline added by `console.log` is NOT counted,
// consistent with the payload the user supplied.
function __taida_stdout(...args) {
  let total = 0;
  for (const arg of args) {
    let rendered;
    if (__taida_isBytes(arg)) {
      rendered = __taida_bytes_to_string(arg);
    } else if (__taida_isEnumVal(arg)) {
      // C18-2: Enum wrapper — print ordinal Str (matches interpreter
      // `Value::EnumVal` display which falls back to the ordinal to
      // preserve the `.toString()` contract from C16 / ROOT-4).
      rendered = String(arg.__taida_enum_ordinal);
    } else if (Array.isArray(arg)) {
      rendered = '@[' + arg.map(x => __taida_format(x)).join(', ') + ']';
    } else if (arg && arg.__type === 'Async') {
      const status = arg.status;
      if (status === 'fulfilled') {
        rendered = 'Async[fulfilled: ' + String(arg.__value) + ']';
      } else if (status === 'rejected') {
        rendered = 'Async[rejected: ' + String(arg.__error) + ']';
      } else {
        rendered = 'Async[pending]';
      }
    } else if (arg && arg.__type === 'Result') {
      if (arg.isSuccess()) rendered = 'Result[' + String(arg.__value) + ']';
      else rendered = 'Result(throw)';
    } else if (arg && arg.__type === 'Lax') {
      // Match interpreter BuchiPack display format
      const _lhv = typeof arg.hasValue === 'function' ? arg.hasValue() : arg.hasValue;
      rendered = '@(hasValue <= ' + String(!!_lhv) + ', __value <= ' + __taida_format(arg.__value) + ', __default <= ' + __taida_format(arg.__default) + ', __type <= "Lax")';
    } else if (arg && typeof arg === 'object' && !Array.isArray(arg)) {
      // BuchiPack-like object
      const entries = Object.entries(arg).filter(([k]) => !k.startsWith('__'));
      const formatted = entries.map(([k, v]) => k + ' <= ' + __taida_format(v)).join(', ');
      rendered = '@(' + formatted + ')';
    } else {
      rendered = typeof arg === 'boolean' ? (arg ? 'true' : 'false') : String(arg);
    }
    console.log(rendered);
    // UTF-8 byte length: fall back to TextEncoder when available, otherwise
    // compute via Buffer (Node) — stay parity-exact with Rust `.len()`.
    total += __taida_utf8_byte_length(rendered);
  }
  return total;
}

function __taida_utf8_byte_length(s) {
  if (typeof TextEncoder !== 'undefined') {
    return new TextEncoder().encode(s).length;
  }
  if (typeof Buffer !== 'undefined') {
    return Buffer.byteLength(s, 'utf8');
  }
  // Fallback: manual UTF-8 byte counting
  let n = 0;
  for (let i = 0; i < s.length; i++) {
    const code = s.charCodeAt(i);
    if (code < 0x80) n += 1;
    else if (code < 0x800) n += 2;
    else if (code >= 0xD800 && code <= 0xDBFF) { n += 4; i++; }
    else n += 3;
  }
  return n;
}

function __taida_format(v) {
  if (typeof v === 'string') return '"' + v + '"';
  if (__taida_isBytes(v)) return __taida_bytes_to_string(v);
  // C18-2: Enum wrapper — format as its ordinal Int to match the
  // interpreter's `to_debug_string` for `Value::EnumVal`.
  if (__taida_isEnumVal(v)) return String(v.__taida_enum_ordinal);
  if (Array.isArray(v)) return '@[' + v.map(x => __taida_format(x)).join(', ') + ']';
  if (typeof v === 'boolean') return v ? 'true' : 'false';
  if (v && typeof v === 'object' && !Array.isArray(v) && !v.__type) {
    const entries = Object.entries(v).filter(([k]) => !k.startsWith('__'));
    return '@(' + entries.map(([k, val]) => k + ' <= ' + __taida_format(val)).join(', ') + ')';
  }
  return String(v);
}

// C12-2b: Taida `.toString()` dispatch. Matches the interpreter's
// `Value::to_display_string()`: strings round-trip unquoted, BuchiPacks
// render as `@(field <= value, ...)`, lists as `@[...]`, primitives as
// their natural string form. Typed packs (Result/Lax/HashMap/Set/etc.)
// delegate to their own `toString()` prototype methods set up above.
function __taida_to_string(v) {
  if (typeof v === 'string') return v;
  if (typeof v === 'number') return String(v);
  if (typeof v === 'boolean') return v ? 'true' : 'false';
  if (v === null || v === undefined) return '';
  if (__taida_isBytes(v)) return __taida_bytes_to_string(v);
  // C18-2: Enum wrapper (`__taida_enumVal`) — `.toString()` returns the
  // ordinal Str (preserving the interpreter's Display contract). The
  // variant-name Str is only used by jsonEncode via `toJSON`.
  if (__taida_isEnumVal(v)) return String(v.__taida_enum_ordinal);
  if (Array.isArray(v)) {
    return '@[' + v.map(x => __taida_format(x)).join(', ') + ']';
  }
  if (typeof v === 'object') {
    // Typed packs (Result, Lax, HashMap, Set, Gorillax, etc.) carry a
    // bespoke toString prototype method — prefer that for parity with
    // the interpreter's typed dispatch.
    if (v.__type && typeof v.toString === 'function' && v.toString !== Object.prototype.toString) {
      return String(v.toString());
    }
    const entries = Object.entries(v).filter(([k]) => !k.startsWith('__'));
    return '@(' + entries.map(([k, val]) => k + ' <= ' + __taida_format(val)).join(', ') + ')';
  }
  return String(v);
}

// ── stderr — Taida error output function (prelude) ──────
// C12-5 (FB-18): returns total UTF-8 byte length of the written content (Int),
// mirroring `__taida_stdout`. Emits a trailing newline to match interpreter
// `eprintln!("{}", ...)` output, but that newline is NOT counted.
function __taida_stderr(...args) {
  let total = 0;
  for (const a of args) {
    const s = String(a);
    process.stderr.write(s + '\n');
    total += __taida_utf8_byte_length(s);
  }
  return total;
}

// ── stdin — Taida input function (prelude) ───────────────
// ESM: node:fs is loaded via top-level await dynamic import (no require())
const __taida_fs = await import('node:fs').catch(() => null);

function __taida_stdin(prompt) {
  if (typeof globalThis.process !== 'undefined' && __taida_fs) {
    if (prompt) process.stdout.write(prompt);
    try {
      const buf = Buffer.alloc(1024); let line = '';
      // Use fd 0 (process.stdin.fd) for cross-platform compatibility.
      // Use fd instead of /dev/stdin for Windows compatibility.
      // Note: Do NOT close this fd — it is process.stdin.fd, not a newly opened handle.
      const fd = process.stdin.fd ?? 0;
      let n; while ((n = __taida_fs.readSync(fd, buf, 0, 1)) > 0) {
        const ch = buf.toString('utf-8', 0, n); if (ch === '\n') break; line += ch;
      }
      return line.replace(/\r$/, '');
    } catch(e) { return ''; }
  }
  return '';
}

const __TAIDA_MAX_SLEEP_MS = 2147483647;

// ── Time prelude functions (minimal kernel) ───────────────
function __taida_nowMs() {
  return Date.now();
}

function __taida_sleep(ms) {
  if (!Number.isInteger(ms)) {
    return new __TaidaAsync(
      null,
      new __TaidaError('TypeError', `sleep: ms must be Int, got ${String(ms)}`, {}),
      'rejected'
    );
  }
  if (ms < 0 || ms > __TAIDA_MAX_SLEEP_MS) {
    return new __TaidaAsync(
      null,
      new __TaidaError(
        'RangeError',
        `sleep: ms must be in range 0..=${__TAIDA_MAX_SLEEP_MS}, got ${ms}`,
        {}
      ),
      'rejected'
    );
  }
  const promise = new Promise((resolve) => {
    setTimeout(() => resolve(Object.freeze({})), ms);
  });
  return __taida_async_pending_from_promise(promise);
}

// ── JSON prelude functions (output-direction only) ──────
function __taida_jsonEncode(v) {
  if (v instanceof __TaidaJSON) return JSON.stringify(__taidaSortKeys(v.__value));
  return JSON.stringify(__taidaSortKeys(v));
}

function __taida_jsonPretty(v) {
  if (v instanceof __TaidaJSON) return JSON.stringify(__taidaSortKeys(v.__value), null, 2);
  return JSON.stringify(__taidaSortKeys(v), null, 2);
}

// ── Safe unmold helper ───────────────────────────────────
// Unwrap mold types or return the value as-is.
// JSON is opaque — cannot be unmolded directly.
function __taida_unmold(v) {
  if (v instanceof __TaidaJSON) throw new __TaidaError('TypeError', 'Cannot unmold JSON directly. Use JSON[raw, Schema]() first.', {});
  if (v instanceof __TaidaAsync) {
    if (v.status === 'rejected') throw v.__error;
    return v.__value;
  }
  if (v && typeof v === 'object') {
    // Stream unmold: collect all items
    if (v.__type === 'Stream') return __taida_stream_collect(v);
    // Lax unmold
    if (v.__type === 'Lax') {
      const hv = typeof v.hasValue === 'function' ? v.hasValue() : v.hasValue;
      return hv ? v.__value : v.__default;
    }
    // Result unmold — evaluate predicate if present
    if (v.__type === 'Result') {
      if (typeof v.unmold === 'function') return v.unmold();
      // Fallback for raw objects without .unmold()
      if (v.throw !== null && v.throw !== undefined) {
        if (v.throw && typeof v.throw === 'object') {
          throw new __TaidaError(v.throw.type || 'ResultError', v.throw.message || String(v.throw), {});
        }
        throw v.throw;
      }
      return v.__value;
    }
    // TODO unmold: return unm channel (fallback __default, then __value).
    if (v.__type === 'TODO') {
      if (Object.prototype.hasOwnProperty.call(v, 'unm')) return v.unm;
      if (Object.prototype.hasOwnProperty.call(v, '__default')) return v.__default;
      if (Object.prototype.hasOwnProperty.call(v, '__value')) return v.__value;
      return Object.freeze({});
    }
    // Molten is opaque — cannot be unmolded directly.
    if (v.__type === 'Molten') {
      throw new __TaidaError('TypeError', 'Cannot unmold Molten directly. Molten can only be used inside Cage.', {});
    }
    // Gorillax unmold: success → value, failure → gorilla (exit)
    if (v.__type === 'Gorillax') {
      const hv = typeof v.hasValue === 'function' ? v.hasValue() : v.hasValue;
      if (hv) return v.__value;
      if (typeof process !== 'undefined') process.exit(1);
      throw new __NativeError('><');
    }
    // RelaxedGorillax unmold: success → value, failure → throw (catchable)
    if (v.__type === 'RelaxedGorillax') {
      const hv = typeof v.hasValue === 'function' ? v.hasValue() : v.hasValue;
      if (hv) return v.__value;
      throw new __TaidaError('RelaxedGorillaEscaped', 'Relaxed gorilla escaped', {});
    }
  }
  return v;
}

// Async version of __taida_unmold — handles true Promises (Phase 2 async OS API).
// Used in async contexts (top-level ESM + async functions) via `await __taida_unmold_async(...)`.
async function __taida_unmold_async(v) {
  if (v && typeof v.then === 'function') {
    // Promise-based OS APIs already resolve to monadic objects (Lax/Result).
    // Do not unmold again after awaiting, or `]=>` loses one level too many.
    return await v;
  }
  return __taida_unmold(v);
}

// ── Structural equality helper ───────────────────────────
// Taida uses structural equality (value-based) not reference identity.
function __taida_equals(a, b) {
  if (a === b) return true;
  if (a == null || b == null) return false;
  // C18-2: Enum-aware equality. Enum wrappers compare by (enum_name,
  // ordinal). An Enum compares equal to a plain Number with the same
  // ordinal, mirroring the interpreter's `Int ↔ EnumVal` compatibility.
  const aIsEnum = __taida_isEnumVal(a);
  const bIsEnum = __taida_isEnumVal(b);
  if (aIsEnum && bIsEnum) {
    return a.__taida_enum_name === b.__taida_enum_name
      && a.__taida_enum_ordinal === b.__taida_enum_ordinal;
  }
  if (aIsEnum && typeof b === 'number') return a.__taida_enum_ordinal === b;
  if (bIsEnum && typeof a === 'number') return b.__taida_enum_ordinal === a;
  if (typeof a !== typeof b) return false;
  if (typeof a !== 'object') return a === b;
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) return false;
    return a.every((v, i) => __taida_equals(v, b[i]));
  }
  if (Array.isArray(a) || Array.isArray(b)) return false;
  // Filter out internal keys (__*) and method keys (function values)
  const ka = Object.keys(a).filter(k => !k.startsWith('__') && typeof a[k] !== 'function');
  const kb = Object.keys(b).filter(k => !k.startsWith('__') && typeof b[k] !== 'function');
  if (ka.length !== kb.length) return false;
  return ka.every(k => __taida_equals(a[k], b[k]));
}

// ── List/String/Number method extensions ─────────────────
// Only state-check methods, safe accessors, and monadic ops remain as prototype methods.
// All operation methods have been moved to standalone mold functions above.
if (!Array.prototype.__taida_patched) {
  Object.defineProperty(Array.prototype, '__taida_patched', { value: true, enumerable: false });
  Object.defineProperty(Array.prototype, 'length_', {
    value: function() { return this.length; }, enumerable: false
  });
  Object.defineProperty(Array.prototype, 'first', {
    value: function() {
      if (this.length === 0) return Lax(null);
      return Lax(this[0]);
    }, enumerable: false
  });
  Object.defineProperty(Array.prototype, 'last', {
    value: function() {
      if (this.length === 0) return Lax(null);
      return Lax(this[this.length - 1]);
    }, enumerable: false
  });
  Object.defineProperty(Array.prototype, 'contains', {
    value: function(v) { return this.some(x => __taida_equals(x, v)); }, enumerable: false
  });
  // Override Array.prototype.indexOf with structural equality for Taida
  Object.defineProperty(Array.prototype, 'indexOf', {
    value: function(v) {
      for (let i = 0; i < this.length; i++) {
        if (__taida_equals(this[i], v)) return i;
      }
      return -1;
    }, enumerable: false, configurable: true
  });
  // lastIndexOf(val) — last index of element using structural equality
  Object.defineProperty(Array.prototype, 'lastIndexOf', {
    value: function(v) {
      for (let i = this.length - 1; i >= 0; i--) {
        if (__taida_equals(this[i], v)) return i;
      }
      return -1;
    }, enumerable: false, configurable: true
  });
  // any(fn) — true if any element satisfies the predicate
  Object.defineProperty(Array.prototype, 'any', {
    value: function(fn) {
      for (let i = 0; i < this.length; i++) {
        if (fn(this[i]) === true) return true;
      }
      return false;
    }, enumerable: false
  });
  // all(fn) — true if all elements satisfy the predicate
  Object.defineProperty(Array.prototype, 'all', {
    value: function(fn) {
      for (let i = 0; i < this.length; i++) {
        if (fn(this[i]) !== true) return false;
      }
      return true;
    }, enumerable: false
  });
  // none(fn) — true if no element satisfies the predicate
  Object.defineProperty(Array.prototype, 'none', {
    value: function(fn) {
      for (let i = 0; i < this.length; i++) {
        if (fn(this[i]) === true) return false;
      }
      return true;
    }, enumerable: false
  });
  // get(index, customDefault?) — return Lax
  Object.defineProperty(Array.prototype, 'get', {
    value: function(idx, customDefault) {
      if (idx >= 0 && idx < this.length) {
        const val = this[idx];
        const lax = Lax(val);
        if (customDefault !== undefined) {
          return Object.freeze({
            __type: 'Lax',
            __value: val,
            __default: customDefault,
            hasValue: __taida_hasValue(true),
            isEmpty() { return false; },
            getOrDefault(def) { return val; },
            map(fn) { return Lax(fn(val)); },
            flatMap(fn) { const r = fn(val); return r && r.__type === 'Lax' ? r : Lax(r); },
            unmold() { return val; },
            toString() { return 'Lax(' + String(val) + ')'; },
          });
        }
        return lax;
      }
      const def = customDefault !== undefined ? customDefault : (this.length > 0 ? __taida_lax_default(this[0]) : 0);
      return Object.freeze({
        __type: 'Lax',
        __value: def,
        __default: def,
        hasValue: __taida_hasValue(false),
        isEmpty() { return true; },
        getOrDefault(d) { return d; },
        map(fn) { return this; },
        flatMap(fn) { return this; },
        unmold() { return def; },
        toString() { return 'Lax(default: ' + String(def) + ')'; },
      });
    }, enumerable: false
  });
  // isEmpty() — check if list is empty
  Object.defineProperty(Array.prototype, 'isEmpty', {
    value: function() { return this.length === 0; }, enumerable: false
  });
  // max() — return Lax (empty list returns Lax with hasValue=false)
  Object.defineProperty(Array.prototype, 'max', {
    value: function() {
      if (this.length === 0) return Lax(null);
      return Lax(this.reduce((a, b) => a > b ? a : b));
    }, enumerable: false
  });
  // min() — return Lax (empty list returns Lax with hasValue=false)
  Object.defineProperty(Array.prototype, 'min', {
    value: function() {
      if (this.length === 0) return Lax(null);
      return Lax(this.reduce((a, b) => a < b ? a : b));
    }, enumerable: false
  });
  Object.defineProperty(Array.prototype, 'toString', {
    value: function() { return '@[' + this.map(x => typeof x === 'string' ? '"' + x + '"' : String(x)).join(', ') + ']'; }, enumerable: false
  });
}

// Taida calls .toString() on numbers, booleans, etc.
// Number/Boolean already have toString, but ensure Taida-compatible formatting.
if (!Number.prototype.__taida_patched) {
  Object.defineProperty(Number.prototype, '__taida_patched', { value: true, enumerable: false });
  Object.defineProperty(Number.prototype, 'isNaN', {
    value: function() { return Number.isNaN(this.valueOf()); }, enumerable: false
  });
  Object.defineProperty(Number.prototype, 'isInfinite', {
    value: function() { const v = this.valueOf(); return !Number.isFinite(v) && !Number.isNaN(v); }, enumerable: false
  });
  Object.defineProperty(Number.prototype, 'isFinite', {
    value: function() { return Number.isFinite(this.valueOf()); }, enumerable: false
  });
  Object.defineProperty(Number.prototype, 'isPositive', {
    value: function() { return this.valueOf() > 0; }, enumerable: false
  });
  Object.defineProperty(Number.prototype, 'isNegative', {
    value: function() { return this.valueOf() < 0; }, enumerable: false
  });
  Object.defineProperty(Number.prototype, 'isZero', {
    value: function() { return this.valueOf() === 0; }, enumerable: false
  });
}

if (typeof Uint8Array !== 'undefined' && !Uint8Array.prototype.__taida_bytes_patched) {
  Object.defineProperty(Uint8Array.prototype, '__taida_bytes_patched', { value: true, enumerable: false });
  Object.defineProperty(Uint8Array.prototype, 'length_', {
    value: function() { return this.length; }, enumerable: false
  });
  Object.defineProperty(Uint8Array.prototype, 'get', {
    value: function(idx) {
      if (__taida_isIntNumber(idx) && idx >= 0 && idx < this.length) return Lax(Number(this[idx]));
      return Object.freeze({
        __type: 'Lax', __value: 0, __default: 0, hasValue: __taida_hasValue(false),
        isEmpty() { return true; }, getOrDefault(d) { return d; },
        map(fn) { return this; }, flatMap(fn) { return this; },
        unmold() { return 0; }, toString() { return 'Lax(default: 0)'; },
      });
    }, enumerable: false
  });
  Object.defineProperty(Uint8Array.prototype, 'toString', {
    value: function() { return __taida_bytes_to_string(this); }, enumerable: false, configurable: true
  });
}

// String methods: only state-check and safe access methods remain.
// Operation methods (reverse, trim, etc.) are now standalone mold functions.
if (!String.prototype.__taida_str_patched) {
  // Save native startsWith/endsWith BEFORE overwriting
  const __native_startsWith = String.prototype.startsWith;
  const __native_endsWith = String.prototype.endsWith;
  Object.defineProperty(String.prototype, '__taida_str_patched', { value: true, enumerable: false });
  Object.defineProperty(String.prototype, 'length_', {
    value: function() { return this.length; }, enumerable: false
  });
  Object.defineProperty(String.prototype, 'contains', {
    value: function(v) { return this.includes(v); }, enumerable: false
  });
  // startsWith/endsWith — delegate to saved native references
  Object.defineProperty(String.prototype, 'startsWith', {
    value: function(v) { return __native_startsWith.call(this, v); }, enumerable: false, configurable: true
  });
  Object.defineProperty(String.prototype, 'endsWith', {
    value: function(v) { return __native_endsWith.call(this, v); }, enumerable: false, configurable: true
  });
  // indexOf — already native, structural for string is identity
  // lastIndexOf — already native
  // get(index) — return Lax for string character access
  Object.defineProperty(String.prototype, 'get', {
    value: function(idx) {
      if (idx >= 0 && idx < this.length) return Lax(this[idx]);
      return Object.freeze({
        __type: 'Lax', __value: '', __default: '', hasValue: __taida_hasValue(false),
        isEmpty() { return true; }, getOrDefault(d) { return d; },
        map(fn) { return this; }, flatMap(fn) { return this; },
        unmold() { return ''; }, toString() { return 'Lax(default: "")'; },
      });
    }, enumerable: false
  });
}

// ── Helper: sort object keys for deterministic JSON output ──
function __taidaSortKeys(obj) {
  // C18-2: Pass Enum wrappers through untouched so JSON.stringify invokes
  // their `toJSON` method and emits the variant-name Str.
  if (__taida_isEnumVal(obj)) return obj;
  if (Array.isArray(obj)) return obj.map(__taidaSortKeys);
  if (obj && typeof obj === 'object' && !(obj instanceof __TaidaJSON)) {
    const sorted = {};
    for (const k of Object.keys(obj).sort()) {
      // Skip __type — internal metadata, not user data
      if (k === '__type') continue;
      sorted[k] = __taidaSortKeys(obj[k]);
    }
    return sorted;
  }
  return obj;
}

// ── __taida_std removed (std dissolution) ─────────────────
// stdout/stderr/stdin: __taida_stdout/__taida_stderr/__taida_stdin
// time: __taida_nowMs/__taida_sleep (minimal kernel)
// JSON: __taida_jsonEncode/__taida_jsonPretty (output-direction only)
// jsonParse/jsonDecode/jsonFrom: ABOLISHED (Molten Iron)

// ── Prelude utility functions ─────────────────────────────

// ── HashMap — immutable key-value collection ──
// Internal: entries = [{key, value}, ...] (frozen BuchiPack pairs)
// All mutating methods return a new HashMap.
function __taida_createHashMap(entries) {
  const _entries = Object.freeze(entries);
  const hm = {
    __type: 'HashMap',
    __entries: _entries,
    get(key) {
      for (const e of _entries) {
        if (__taida_equals(e.key, key)) return Lax(e.value);
      }
      return Lax(undefined);
    },
    set(key, value) {
      const newEntries = [];
      let found = false;
      for (const e of _entries) {
        if (__taida_equals(e.key, key)) {
          newEntries.push(Object.freeze({ key, value }));
          found = true;
        } else {
          newEntries.push(e);
        }
      }
      if (!found) newEntries.push(Object.freeze({ key, value }));
      return __taida_createHashMap(newEntries);
    },
    remove(key) {
      return __taida_createHashMap(_entries.filter(e => !__taida_equals(e.key, key)));
    },
    has(key) {
      return _entries.some(e => __taida_equals(e.key, key));
    },
    keys() {
      return Object.freeze(_entries.map(e => e.key));
    },
    values() {
      return Object.freeze(_entries.map(e => e.value));
    },
    entries() {
      return Object.freeze(_entries.map(e => Object.freeze({ first: e.key, second: e.value })));
    },
    size() {
      return _entries.length;
    },
    isEmpty() {
      return _entries.length === 0;
    },
    merge(other) {
      if (!other || other.__type !== 'HashMap') return hm;
      const merged = [..._entries];
      for (const oe of other.__entries) {
        const idx = merged.findIndex(e => __taida_equals(e.key, oe.key));
        if (idx >= 0) {
          merged[idx] = oe;
        } else {
          merged.push(oe);
        }
      }
      return __taida_createHashMap(merged);
    },
    // Format: `HashMap({key1: val1, key2: val2})` — matches interpreter output.
    // String keys/values are quoted with `"` and escaped via __taida_escape_str
    // (handles `"`, `\n`, `\t`, `\\`). Non-string values use String() coercion.
    toString() {
      const pairs = _entries.map(e => {
        const k = typeof e.key === 'string' ? '"' + __taida_escape_str(e.key) + '"' : String(e.key);
        const v = typeof e.value === 'string' ? '"' + __taida_escape_str(e.value) + '"' : String(e.value);
        return k + ': ' + v;
      });
      return 'HashMap({' + pairs.join(', ') + '})';
    },
  };
  return Object.freeze(hm);
}

function hashMap(entries) {
  if (Array.isArray(entries)) {
    const parsed = [];
    for (const entry of entries) {
      if (entry && typeof entry === 'object') {
        const key = entry.key !== undefined ? entry.key : (entry.first !== undefined ? entry.first : undefined);
        const value = entry.value !== undefined ? entry.value : (entry.second !== undefined ? entry.second : undefined);
        if (key !== undefined && value !== undefined) {
          parsed.push(Object.freeze({ key, value }));
        }
      }
    }
    return __taida_createHashMap(parsed);
  }
  // BuchiPack (plain object): each field becomes a key-value entry
  // hashMap(@(a <= 1, b <= 2)) -> [{key: "a", value: 1}, {key: "b", value: 2}]
  if (entries && typeof entries === 'object' && !Array.isArray(entries)) {
    const parsed = [];
    for (const [k, v] of Object.entries(entries)) {
      if (k !== '__type') {
        parsed.push(Object.freeze({ key: k, value: v }));
      }
    }
    return __taida_createHashMap(parsed);
  }
  return __taida_createHashMap([]);
}

// ── Set — immutable unique collection ──
// Internal: items = [...unique values] (frozen array)
// All mutating methods return a new Set.
function __taida_createSet(items) {
  const _items = Object.freeze(items);
  const s = {
    __type: 'Set',
    __items: _items,
    add(item) {
      if (_items.some(x => __taida_equals(x, item))) return s;
      return __taida_createSet([..._items, item]);
    },
    remove(item) {
      return __taida_createSet(_items.filter(x => !__taida_equals(x, item)));
    },
    has(item) {
      return _items.some(x => __taida_equals(x, item));
    },
    union(other) {
      if (!other || other.__type !== 'Set') return s;
      const result = [..._items];
      for (const item of other.__items) {
        if (!result.some(x => __taida_equals(x, item))) result.push(item);
      }
      return __taida_createSet(result);
    },
    intersect(other) {
      if (!other || other.__type !== 'Set') return __taida_createSet([]);
      return __taida_createSet(_items.filter(item => other.__items.some(x => __taida_equals(x, item))));
    },
    diff(other) {
      if (!other || other.__type !== 'Set') return s;
      return __taida_createSet(_items.filter(item => !other.__items.some(x => __taida_equals(x, item))));
    },
    toList() {
      return _items;
    },
    size() {
      return _items.length;
    },
    isEmpty() {
      return _items.length === 0;
    },
    // Format: `Set({val1, val2})` — matches interpreter output.
    // String items are quoted with `"` and escaped via __taida_escape_str.
    // Non-string items use String() coercion, consistent with HashMap.toString().
    toString() {
      const strs = _items.map(i => typeof i === 'string' ? '"' + __taida_escape_str(i) + '"' : String(i));
      return 'Set({' + strs.join(', ') + '})';
    },
  };
  return Object.freeze(s);
}

function setOf(items) {
  if (!Array.isArray(items)) return __taida_createSet([]);
  const unique = [];
  for (const item of items) {
    if (!unique.some(x => __taida_equals(x, item))) unique.push(item);
  }
  return __taida_createSet(unique);
}

function range(start, end) {
  if (end === undefined) { end = start; start = 0; }
  const result = [];
  for (let i = start; i < end; i++) result.push(i);
  return Object.freeze(result);
}

function enumerate(list) {
  if (!Array.isArray(list)) return Object.freeze([]);
  return Object.freeze(list.map((value, index) => Object.freeze({ index, value })));
}

function zip(a, b) {
  if (!Array.isArray(a) || !Array.isArray(b)) return Object.freeze([]);
  const len = Math.min(a.length, b.length);
  const result = [];
  for (let i = 0; i < len; i++) {
    result.push(Object.freeze({ first: a[i], second: b[i] }));
  }
  return Object.freeze(result);
}

function __taida_assert(cond, msg) {
  if (!cond) throw new __TaidaError('AssertionError', msg || 'Assertion failed', {});
}

function __taida_list_method_removed(method) {
  throw new __TaidaError(
    'MethodError',
    `List method .${method}() has moved to molds. Use molds such as Join[], Sum[], Reverse[], Sort[] instead.`,
    {}
  );
}

function __taida_typeof(x) {
  if (x === null || x === undefined) return 'Unit';
  if (__taida_isBytes(x)) return 'Bytes';
  if (typeof x === 'bigint') return 'Int';
  if (typeof x === 'number') return Number.isInteger(x) ? 'Int' : 'Float';
  if (typeof x === 'boolean') return 'Bool';
  if (typeof x === 'string') return 'Str';
  if (Array.isArray(x)) return 'List';
  if (x instanceof __TaidaJSON) return 'JSON';
  if (x instanceof __TaidaAsync) return 'Async';
  if (x && x.__type) return x.__type;
  if (typeof x === 'object') return 'BuchiPack';
  return 'Unknown';
}

// ── JS interop helpers (Molten operations) ──
function __taida_js_spread(target, source) {
  if (Array.isArray(target)) {
    return [...target, ...(Array.isArray(source) ? source : [source])];
  }
  return {...target, ...source};
}
"#;
