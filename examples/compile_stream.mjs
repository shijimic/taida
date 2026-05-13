
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

// ── C21-5 / seed-04: Float-origin formatting helpers ─────
//
// JS `Number` cannot distinguish `12` from `12.0` at runtime. The Taida
// interpreter (reference) renders `Value::Float(12.0)` as `"12.0"` and
// `Value::Int(12)` as `"12"`. To preserve 3-backend parity without
// wrapping every Number at runtime (which would deopt arithmetic), the
// JS codegen performs compile-time Float-origin analysis and, at
// terminal sites (`stdout` / `debug` / `.toString()`) where the
// expression is known to be Float-origin, emits these `_f` variants.
//
// Non-number values (Str, Bool, BuchiPack, List, Bytes, Lax/Result
// wrappers, etc.) fall through to the same formatting path as the
// non-`_f` helpers so parity is preserved for mixed-type output.
function __taida_float_render(v) {
  if (typeof v === 'number') {
    // C26B-011 (Phase 11): IEEE 754 special values — match Rust's
    // `f64::Display` (what the interpreter uses via `n.to_string()`):
    //   NaN         → "NaN"
    //   +Infinity   → "inf"
    //   -Infinity   → "-inf"
    // JS' default `String(v)` produces "NaN" (OK) but "Infinity" /
    // "-Infinity" which drifts from interpreter and native. Native
    // already routes through `taida_float_to_str` for the same mapping
    // (see `src/codegen/native_runtime/core.c::taida_float_to_str`).
    if (Number.isNaN(v)) return 'NaN';
    if (v === Infinity) return 'inf';
    if (v === -Infinity) return '-inf';
    if (Number.isFinite(v) && Number.isInteger(v)) {
      // C26B-011 / Round 7 wV-a: IEEE-754 signed zero. `Number.isInteger(-0)`
      // is true, and `(-0).toFixed(1)` returns "0.0" (sign drops),
      // diverging from the interpreter's `format!("{:.1}", -0.0)` which
      // yields "-0.0". Detect the sign bit explicitly via `Object.is`
      // before falling through to `.toFixed(1)`.
      if (Object.is(v, -0)) return '-0.0';
      // Match Rust's `format!("{:.1}", n)` used by the interpreter.
      return v.toFixed(1);
    }
  }
  return String(v);
}

function __taida_stdout_f(v) {
  // Float-origin specialisation of __taida_stdout for a single value.
  // Falls back to the generic stdout renderer for non-number values so
  // a Float-returning function that yields NaN / Inf / Error / etc.
  // still prints sensibly.
  let rendered;
  if (typeof v === 'number') {
    rendered = __taida_float_render(v);
  } else if (__taida_isBytes(v)) {
    rendered = __taida_bytes_to_string(v);
  } else {
    // Delegate to the full stdout path for BuchiPack / Array / typed
    // wrappers. Re-entering __taida_stdout is safe because `v` is not
    // a number here so the specialisation never recurses.
    return __taida_stdout(v);
  }
  console.log(rendered);
  return __taida_utf8_byte_length(rendered);
}

function __taida_debug_f(v) {
  if (typeof v === 'number') {
    console.log(__taida_float_render(v));
    return;
  }
  return __taida_debug(v);
}

function __taida_to_string_f(v) {
  if (typeof v === 'number') {
    return __taida_float_render(v);
  }
  return __taida_to_string(v);
}

// C21-5 / seed-04: runtime fallback for Int[x]()/Float[x]() when the
// arg is not a compile-time-known FloatLit / IntLit. Matches the
// existing `Number.isInteger`-based behaviour — dynamic cases remain
// best-effort per design (closure-crossing is out of scope).
function __taida_is_int(v) {
  return typeof v === 'number' && Number.isFinite(v) && Number.isInteger(v);
}

function __taida_is_float(v) {
  return typeof v === 'number' && Number.isFinite(v) && !Number.isInteger(v);
}

// C21B-seed-04 re-fix (2026-04-22): `Float[x]()` in Taida always produces
// a Float-typed Lax, regardless of input representation. JS Number has no
// Int/Float tag, so an integer-valued Float (e.g. `Float[3]()` yielding
// `Lax[3.0]`) was rendering as `Lax[3]`. Here we build the Lax with a
// `__floatHint: true` marker so the stdout/debug/toString formatters use
// Float-aware rendering (`3.0`, `0.0`) for `__value` and `__default`.
// Always used when the codegen sees `Float[...]()` — this is purely a
// display-side tag; arithmetic and equality paths stay untouched (no
// deopt). Matches the interpreter's `Value::Float` tag.
function Float_mold_f(value) {
  let num;
  if (typeof value === 'number') num = value;
  else if (typeof value === 'bigint') num = Number(value);
  else if (typeof value === 'boolean') num = value ? 1.0 : 0.0;
  else if (typeof value === 'string') {
    const f = parseFloat(value);
    if (isNaN(f)) {
      return Lax(null, 0.0, true);
    }
    num = f;
  } else {
    return Lax(null, 0.0, true);
  }
  return Lax(num, 0.0, true);
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
  // C26B-011 / Round 7 wV-a: IEEE-754 signed-zero preservation for
  // Float arithmetic. When either operand already carries the
  // negative-zero sign bit, or when the Number-path product is -0
  // (e.g. `-1 * 0 === -0`), routing through the BigInt fast-path
  // would collapse the sign (BigInt has no -0). Stay on the Number
  // multiplication path in those cases so `__taida_float_render`
  // can observe the sign bit via `Object.is(v, -0)` and render
  // "-0.0", matching the interpreter's `format!("{:.1}", -0.0)`.
  if (__taida_isIntNumber(a) && __taida_isIntNumber(b)) {
    const prod = a * b;
    if (Object.is(prod, -0) || Object.is(a, -0) || Object.is(b, -0)) {
      return prod;
    }
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

function __taida_lax_from_bytes(bytes, hasValue, error) {
  const val = bytes instanceof Uint8Array ? new Uint8Array(bytes) : new Uint8Array(0);
  const _hasValue = !!hasValue;
  const _error = error === undefined ? null : error;
  if (_hasValue && _error !== null) {
    throw new __TaidaError('Lax success cannot carry ErrorInfo');
  }
  const pack = {
    __type: 'Lax',
    __value: val,
    __default: new Uint8Array(0),
    has_value: _hasValue,
    hasValue() { return _hasValue; },
    isEmpty() { return !_hasValue; },
    errorInfo() { return __taida_error_info_lax(_error); },
    getOrDefault(def) { return _hasValue ? val : def; },
    map(fn) { return _hasValue ? Lax(fn(val)) : this; },
    flatMap(fn) {
      if (!_hasValue) return this;
      const result = fn(val);
      if (result && result.__type === 'Lax') return result;
      return Lax(result);
    },
    unmold() { return _hasValue ? val : new Uint8Array(0); },
    toString() {
      return _hasValue ? 'Lax(' + __taida_bytes_to_string(val) + ')' : 'Lax(default: ' + __taida_bytes_to_string(new Uint8Array(0)) + ')';
    },
  };
  if (_error !== null) pack.__error = _error;
  return Object.freeze(pack);
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
    // E33B-003 Cat B: lift `fields` to top-level so user code's
    // `err.kind` / `err.code` etc. matches Interpreter / Native parity
    // (where Error fields surface as direct properties via
    // `get_error_field`). Preserve the legacy `err.fields.X` callers by
    // keeping `this.fields` populated as before.
    if (fields) {
      for (const k of Object.keys(fields)) {
        // Do not clobber the canonical `type` / `message` / `name` /
        // `stack` properties already set by the JS Error superclass.
        if (k === 'type' || k === 'message' || k === 'name' || k === 'stack') continue;
        this[k] = fields[k];
      }
    }
  }

  errorInfo() {
    return __taida_error_info_lax(this);
  }
}

// Standalone throw function (no Object.prototype pollution)
function __taida_throw(obj) {
  throw obj instanceof globalThis.Error ? obj : new __TaidaError(obj.type || 'Error', obj.message || '', obj);
}

function __taida_error_info_default() {
  return Object.freeze({
    __type: 'ErrorInfo',
    type: '',
    message: '',
    kind: '',
    code: 0,
  });
}

function __taida_error_info(error) {
  let type = 'Error';
  let message = '';
  let kind = '';
  let code = 0;

  if (error instanceof __TaidaError || error instanceof globalThis.Error) {
    type = error.type || error.name || 'Error';
    message = error.message || '';
    kind = error.kind || type;
    code = Number.isInteger(error.code) ? error.code : 0;
  } else if (error && typeof error === 'object') {
    type = error.type || error.__type || 'Error';
    message = error.message || '';
    kind = error.kind || type;
    code = Number.isInteger(error.code) ? error.code : 0;
  } else if (error !== null && error !== undefined) {
    type = __taida_typeof(error);
    message = String(error);
    kind = type;
  }

  return Object.freeze({
    __type: 'ErrorInfo',
    type: String(type),
    message: String(message),
    kind: String(kind || type),
    code,
  });
}

function __taida_error_info_lax(error) {
  const def = __taida_error_info_default();
  if (error === null || error === undefined) return Lax(null, def);
  return Lax(__taida_error_info(error), def);
}

function __taida_error_pack(type, message, kind, code) {
  const pack = {
    __type: String(type || 'Error'),
    type: String(type || 'Error'),
    message: String(message || ''),
    kind: String(kind || type || 'Error'),
    code: Number.isInteger(code) ? code : 0,
  };
  Object.defineProperties(pack, {
    errorInfo: { value() { return __taida_error_info_lax(pack); }, enumerable: false },
    throw: { value() { throw pack; }, enumerable: false },
    toString: { value() { return pack.message || pack.type; }, enumerable: false },
  });
  return Object.freeze(pack);
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
//   @(has_value=false, __value=Int(0), __default=Int(0), __type="Lax")
// First-variant-is-default rule is encoded via Int(0). Delegates to
// `Lax(null, 0)` so the returned object carries the full Lax method set
// (`has_value`, `getOrDefault`, `isEmpty`, `map`, `flatMap`, `unmold`,
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
      return Lax(null, defaultVal, undefined, Object.freeze({
        __type: 'JsonError',
        type: 'JsonError',
        message: 'JSON parse error: invalid input',
        kind: 'parse',
        code: 0,
      }));
    }
  } else {
    jsonData = __taidaValueToJson(rawValue);
  }

  // Cast through schema
  const typedValue = __taida_castJson(jsonData, schema);
  const defaultVal = __taida_defaultForSchema(schema);
  return Lax(typedValue, defaultVal);
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
  // E30 Phase 6 / E30B-004 (Lock-D verdict): synthetic defaultFn for
  // declare-only function fields. The schema `{ __fn: retSchema }` produces
  // an arrow function that, when called, returns the return-type's default
  // value. Matches the interpreter's `DEFAULT_FN_SENTINEL_NAME` synthetic
  // FuncValue. The function ignores its arguments — arity is enforced at
  // the type level, not at runtime.
  if (schema && schema.__fn !== undefined) {
    const retSchema = schema.__fn;
    return function __taida_default_fn() {
      return __taida_defaultForSchema(retSchema);
    };
  }
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
function __taida_result_create(value, throwVal, predicate, displayOrder) {
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
  const result = {
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
      // Pass the throw payload `P` directly to the mapper so the runtime
      // matches `mapError(fn: P -> Q) -> Result[T, Q]`. Direct-store
      // applies to Error-derived BuchiPacks (those carrying `__type`);
      // anything else is wrapped in a generic ResultError carrier whose
      // message is materialised via `__taida_format` so anonymous packs
      // and primitives round-trip the same string Interpreter/Native
      // produce instead of `[object Object]`.
      const mapped = fn(_throw);
      let newThrow;
      if (mapped && typeof mapped === 'object' && mapped.__type) {
        newThrow = mapped;
      } else {
        const msg = (mapped && typeof mapped === 'object')
          ? __taida_format(mapped)
          : String(mapped);
        newThrow = { __type: 'ResultError', message: msg, type: 'ResultError' };
      }
      return __taida_result_create(null, newThrow, null);
    },
    getOrThrow() {
      if (!_checkError()) return _value;
      if (_throw && typeof _throw === 'object') {
        // E33B-003 Cat B: forward throw object as fields so the
        // __TaidaError constructor lifts top-level keys (e.g. `kind`)
        // for `err.kind` parity with Interpreter / Native.
        throw new __TaidaError(_throw.type || 'ResultError', _throw.message || String(_throw), _throw);
      }
      if (_throw) {
        throw new __TaidaError('ResultError', String(_throw), {});
      }
      // Predicate failed but no explicit throw — generate default error
      throw new __TaidaError('ResultError', 'Result predicate failed for value: ' + String(_value), {});
    },
    toString() {
      if (!_checkError()) return 'Result(' + String(_value) + ')';
      let errDisplay;
      if (_throw && typeof _throw === 'object') {
        // The JS Error factory always materialises `message: ''` for
        // Error-derived packs, so an empty string is indistinguishable
        // from a missing message. Treat `message === ''` as "no
        // message" and fall back to the declared `__type` name; the
        // four backends agree on this even though Interpreter / Native
        // can technically tell the two states apart.
        if (typeof _throw.message === 'string' && _throw.message.length > 0) {
          errDisplay = _throw.message;
        } else if (_throw.__type) {
          errDisplay = _throw.__type;
        } else {
          errDisplay = String(_throw);
        }
      } else if (_throw) {
        errDisplay = String(_throw);
      } else {
        errDisplay = 'predicate failed';
      }
      return 'Result(throw <= ' + errDisplay + ')';
    },
    unmold() {
      if (_checkError()) {
        if (_throw && typeof _throw === 'object') {
          // E33B-003 Cat B: forward throw object as fields (see getOrThrow).
          throw new __TaidaError(_throw.type || 'ResultError', _throw.message || String(_throw), _throw);
        }
        if (_throw) throw _throw;
        // Predicate failed but no explicit throw — generate default error
        throw new __TaidaError('ResultError', 'Result predicate failed for value: ' + String(_value), {});
      }
      return _value;
    },
  };
  if (displayOrder) {
    Object.defineProperty(result, '__taidaResultDisplayOrder', {
      value: displayOrder,
      enumerable: false,
      configurable: false,
    });
  }
  return Object.freeze(result);
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

function Lax(value, typedDefault, floatHint, error) {
  const _hasValue = value !== null && value !== undefined;
  const _default = _hasValue ? __taida_lax_default(value) : (typedDefault !== undefined ? typedDefault : 0);
  const _val = _hasValue ? value : _default;
  // C26B-011 (Phase 11): `floatHint` is forwarded by Float-origin mold
  // callers (e.g. `Div_mold` for Float/Int mixed or Float/Float) so the
  // short-form `.toString()` renders `__value` / `__default` via
  // `__taida_float_render` (which handles `0.0`/`inf`/`-inf`/`NaN`).
  // Without this the default `String(0.0)` collapses to `"0"` and the
  // interpreter's `Lax(default: 0.0)` drifts.
  const _floatHint = floatHint === true;
  // E34 Phase 3 (Lock-D=B'): optional ErrorInfo carrier. Producers like
  // JSON failure / net / file / process pass an `error` describing the
  // failure cause; `errorInfo()` surfaces it as Lax[ErrorInfo]. The
  // `__error` field is **not** materialised on the pack object when no
  // error is recorded — this preserves backwards-compatible JSON
  // serialisation (jsonEncode / __default / __value parity tests must
  // not see a null __error key on success).
  const _incomingError = error === undefined ? null : error;
  if (_hasValue && _incomingError !== null) {
    throw new __TaidaError('StateError', 'Lax success cannot carry ErrorInfo', { kind: 'invalid_state', code: 0 });
  }
  const _error = _hasValue ? null : _incomingError;
  const pack = {
    __type: 'Lax',
    __value: _val,
    __default: _default,
    has_value: _hasValue,
    hasValue() { return _hasValue; },
    isEmpty() { return !_hasValue; },
    errorInfo() { return _hasValue ? __taida_error_info_lax(null) : __taida_error_info_lax(_error); },
    getOrDefault(def) { return _hasValue ? _val : def; },
    map(fn) { return _hasValue ? Lax(fn(_val), undefined, _floatHint) : this; },
    flatMap(fn) {
      if (!_hasValue) return this;
      const result = fn(_val);
      if (result && result.__type === 'Lax') return result;
      return Lax(result, undefined, _floatHint);
    },
    unmold() { return _hasValue ? _val : _default; },
    toString() {
      const fmt = _floatHint ? __taida_float_render : String;
      return _hasValue ? 'Lax(' + fmt(_val) + ')' : 'Lax(default: ' + fmt(_default) + ')';
    },
  };
  if (_floatHint) pack.__floatHint = true;
  if (!_hasValue && _error !== null) pack.__error = _error;
  return Object.freeze(pack);
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
    has_value: _hasValue,
    hasValue() { return _hasValue; },
    isEmpty() { return !_hasValue; },
    errorInfo() { return _hasValue ? __taida_error_info_lax(null) : __taida_error_info_lax(_error); },
    relax() {
      return Object.freeze({
        __type: 'RelaxedGorillax',
        __value: _hasValue ? value : null,
        __error: _error,
        has_value: _hasValue,
        hasValue() { return _hasValue; },
        isEmpty() { return !_hasValue; },
        errorInfo() { return _hasValue ? __taida_error_info_lax(null) : __taida_error_info_lax(_error); },
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

function __taida_cagerilla_runner(fn) {
  Object.defineProperty(fn, '__taida_cagerilla_runner', {
    value: true,
    writable: false,
    configurable: false,
    enumerable: false,
  });
  return fn;
}

// Cage: execute a CageRilla descriptor runner in protected context.
function Cage_mold(cageValue, cageRunner) {
  if (typeof cageRunner !== 'function' || cageRunner.__taida_cagerilla_runner !== true) {
    throw new __TaidaError(
      'TypeError',
      'Cage runner must be a CageRilla descriptor; direct functions and lambdas are not supported',
      {}
    );
  }
  try {
    const result = cageRunner(cageValue);
    return Gorillax(result, null);
  } catch (e) {
    return Gorillax(null, e);
  }
}

// ── Div/Mod Mold types (safe division returning Lax) ──
//
// C26B-011 (Phase 11): propagate `__floatHint` through to the returned
// Lax so `.toString()` renders `0.0` / `inf` / `-inf` / `NaN` per Rust
// f64 Display. Before this fix, `Div[1.0, 2.0]()` returned `Lax(0.5)`
// (OK) but `Div[1.0, 0.0]()` returned `Lax(default: 0)` because the
// default-only path called `Lax(null, def)` without a hint — the
// fallback `String(0.0)` is `"0"` in JS, drifting from interpreter.
function Div_mold(a, b, opts) {
  if (opts === undefined) opts = {};
  const isFloat = !!(opts.__floatHint || (typeof a === 'number' && (!Number.isInteger(a) || (typeof b === 'number' && !Number.isInteger(b)))));
  const def = opts.default !== undefined ? opts.default : (isFloat ? 0.0 : 0);
  if (b === 0) return Lax(null, def, isFloat);
  if (isFloat) return Lax(a / b, undefined, true);
  const result = Number.isInteger(a) && Number.isInteger(b) ? Math.trunc(a / b) : a / b;
  const lax = Lax(result);
  return lax;
}
function Mod_mold(a, b, opts) {
  if (opts === undefined) opts = {};
  const isFloat = !!(opts.__floatHint || (typeof a === 'number' && (!Number.isInteger(a) || (typeof b === 'number' && !Number.isInteger(b)))));
  const def = opts.default !== undefined ? opts.default : (isFloat ? 0.0 : 0);
  if (b === 0) return Lax(null, def, isFloat);
  if (isFloat) return Lax(a % b, undefined, true);
  return Lax(a % b);
}

// ── Type Conversion Mold types (Str/Int/Float/Bool → Lax) ──

// C23-3: Interpreter-parity display string for `Str[x]()`.
// The interpreter implements `Str[x]()` as `format!("{}", other)`, i.e.
// `Value::to_display_string()`, which for a BuchiPack renders ALL fields
// (including `__`-prefixed internals) via each field's `to_debug_string()`.
// The JS backend previously used `String(value)`, which on plain objects
// collapses to `[object Object]` and on typed packs (Lax/Result/…) calls
// their short-form `.toString()` (`Lax(3)`). Both break 4-backend parity
// with the interpreter. This helper mirrors the interpreter contract:
//   Int / Float → natural number string (shortest round-trip, no `.0`)
//   Bool        → `"true"` / `"false"`
//   Str         → unquoted
//   Array       → `@[item0, item1, ...]` (items via `__taida_format`)
//   Lax typed   → `@(has_value <= ..., __value <= ..., __default <= ..., __type <= "Lax")`
//   Result / Async typed → full-form pack (all fields, including `__`-prefixed)
//   HashMap / Set / TODO / Gorillax / RelaxedGorillax → synthetic full-form
//       pack rebuilt from the interpreter's underlying `BuchiPack` layout
//       (C23B-003 reopen — the original C23-3 fix missed these runtime-object
//       types and fell through to the plain-pack branch, which either leaked
//       method source bodies or stripped the `__`-prefixed data the
//       interpreter actually carries in those pack shapes). See
//       `src/interpreter/prelude.rs` (hashMap / setOf) and
//       `src/interpreter/mold_eval.rs` (TODO / Gorillax).
//   Stream       → `Stream[completed: N items]` / `Stream[active]`
//       (`src/interpreter/value.rs:378-381`; interpreter-only type —
//       native/wasm don't support Stream lowering yet).
//   Molten       → `"Molten"` (`src/interpreter/value.rs:377`).
//   Error value  → `Error(type: message)` style (interpreter parity; the
//       JS runtime uses `__TaidaError` instances so this branch only fires
//       for BuchiPack-shaped error wrappers).
//   Plain pack   → `@(field <= value, ...)` (skips `__` fields for user
//       packs, matching the interpreter's `to_display_string` on
//       `Value::BuchiPack` for non-typed packs — the full form only kicks
//       in when `__type` is present, handled above).
// Symmetric with native's `taida_stdout_display_string` and wasm's
// `_wasm_stdout_display_string`.
function __taida_display_string(v) {
  if (typeof v === 'string') return v;
  if (typeof v === 'number') return String(v);
  if (typeof v === 'boolean') return v ? 'true' : 'false';
  if (v === null || v === undefined) return '';
  if (__taida_isBytes(v)) return __taida_bytes_to_string(v);
  if (__taida_isEnumVal(v)) return String(v.__taida_enum_ordinal);
  if (Array.isArray(v)) {
    return '@[' + v.map(x => __taida_format(x)).join(', ') + ']';
  }
  if (typeof v === 'object') {
    // Typed pack: render the full form so the interpreter's
    // `to_display_string()` contract holds (all fields visible, including
    // `__`-prefixed internals). Lax carries an optional `__floatHint` for
    // C21B-seed-04 Float-origin rendering.
    if (v.__type === 'Lax') {
      const _lhv = v.has_value;
      const _fmt = v.__floatHint === true
        ? (n => typeof n === 'number' ? __taida_float_render(n) : __taida_format(n))
        : __taida_format;
      return '@(has_value <= ' + String(!!_lhv)
        + ', __value <= ' + _fmt(v.__value)
        + ', __default <= ' + _fmt(v.__default)
        + ', __type <= "Lax")';
    }
    if (v.__type === 'Result') {
      const pred = v.__predicate ? __taida_format(v.__predicate) : '@()';
      const thrown = (v.throw !== null && v.throw !== undefined) ? __taida_format(v.throw) : '@()';
      if (v.__taidaResultDisplayOrder === 'os') {
        return '@(__value <= ' + __taida_format(v.__value)
          + ', throw <= ' + thrown
          + ', __predicate <= ' + pred
          + ', __type <= "Result")';
      }
      return '@(__value <= ' + __taida_format(v.__value)
        + ', __predicate <= ' + pred
        + ', throw <= ' + thrown
        + ', __type <= "Result")';
    }
    if (v.__type === 'Async') {
      const status = v.status;
      if (status === 'fulfilled') return 'Async[fulfilled: ' + __taida_format(v.__value) + ']';
      if (status === 'rejected') return 'Async[rejected: ' + __taida_format(v.__error) + ']';
      return 'Async[pending]';
    }
    // C23B-003 reopen — HashMap: interpreter represents HashMap as
    // `BuchiPack(__entries <= @[@(key <= K, value <= V), ...], __type <= "HashMap")`
    // (`src/interpreter/prelude.rs:618-621`). `Str[hm]()` therefore renders
    // through `Value::BuchiPack.to_display_string()` = full-form pack.
    // The JS runtime stores HashMap as a frozen object carrying method
    // fields alongside `__entries`, so we must explicitly rebuild the
    // synthetic pack shape instead of iterating the JS object's own keys
    // (which would leak method source bodies as pack fields).
    if (v.__type === 'HashMap') {
      const entries = Array.isArray(v.__entries) ? v.__entries : [];
      const entryStrs = entries.map(e =>
        '@(key <= ' + __taida_format(e.key)
          + ', value <= ' + __taida_format(e.value) + ')'
      );
      return '@(__entries <= @[' + entryStrs.join(', ')
        + '], __type <= "HashMap")';
    }
    // C23B-003 reopen — Set: interpreter `BuchiPack(__items <= @[...], __type <= "Set")`
    // (`src/interpreter/prelude.rs:644-647`).
    if (v.__type === 'Set') {
      const items = Array.isArray(v.__items) ? v.__items : [];
      return '@(__items <= @[' + items.map(x => __taida_format(x)).join(', ')
        + '], __type <= "Set")';
    }
    // C23B-003 reopen — Stream: interpreter uses `Value::Stream` (not a
    // BuchiPack), so `to_display_string()` returns
    // `"Stream[completed: N items]"` / `"Stream[active]"`
    // (`src/interpreter/value.rs:378-381`). Stream lowering has landed
    // on Native / WASM-wasi since C25B-001 Phase 3 (@c.25.rc7), so all
    // 4 backends share this formatting contract. The JS runtime keeps
    // using the `Value::Stream`-shaped object literal (__type /
    // __status / __items) because the JS transpiler lowers Taida
    // `Stream` values to this shape rather than a native Stream
    // primitive — the shape is internal to the JS backend, not a
    // surface concern.
    if (v.__type === 'Stream') {
      if (v.__status === 'active') return 'Stream[active]';
      const items = Array.isArray(v.__items) ? v.__items : [];
      return 'Stream[completed: ' + items.length + ' items]';
    }
    // C23B-003 reopen — TODO mold: interpreter `BuchiPack` with fields
    // `id / task / sol / unm / __value / __default / __type`
    // (`src/interpreter/mold_eval.rs:1793-1801`).
    if (v.__type === 'TODO') {
      return '@(id <= ' + __taida_format(v.id)
        + ', task <= ' + __taida_format(v.task)
        + ', sol <= ' + __taida_format(v.sol)
        + ', unm <= ' + __taida_format(v.unm)
        + ', __value <= ' + __taida_format(v.__value)
        + ', __default <= ' + __taida_format(v.__default)
        + ', __type <= "TODO")';
    }
    // C23B-003 reopen — Gorillax / RelaxedGorillax: interpreter
    // `BuchiPack(has_value, __value, __error, __type)`
    // (`src/interpreter/mold_eval.rs:1824-1829`). The interpreter always
    // emits `__error`; a missing error is `@()` (Value::Unit) — not `null`.
    if (v.__type === 'Gorillax' || v.__type === 'RelaxedGorillax') {
      const hv = v.has_value;
      const err = v.__error;
      // Unit-equivalent absence of error renders as `@()`, matching the
      // interpreter's `Value::Unit.to_display_string()`.
      const errStr = (err === null || err === undefined) ? '@()' : __taida_format(err);
      return '@(has_value <= ' + String(!!hv)
        + ', __value <= ' + __taida_format(v.__value)
        + ', __error <= ' + errStr
        + ', __type <= "' + v.__type + '")';
    }
    // Molten is an opaque interpreter value; `Value::Molten.to_display_string()`
    // returns `"Molten"`.
    if (v.__type === 'Molten') return 'Molten';
    // Plain BuchiPack-like object (user data) — skip `__` internal fields
    // to match the interpreter's `to_display_string` on `Value::BuchiPack`
    // for non-typed packs.
    const entries = Object.entries(v).filter(([k]) => !k.startsWith('__'));
    return '@(' + entries.map(([k, val]) => k + ' <= ' + __taida_format(val)).join(', ') + ')';
  }
  return String(v);
}

function Str_mold(value) {
  // C23-3: route through the interpreter-parity display helper so that
  // Pack / Lax / Result / Async / List values produce full-form Taida
  // display strings rather than JS's `String(value)` coercion
  // (`[object Object]`, `Lax(3)`, …).
  return Lax(__taida_display_string(value));
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
    has_value: false,
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
    has_value: true,
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
// E32B-022 (Lock-N): `searchLax` returns Lax[Int] — has_value=true with
// the char index on a hit, has_value=false (default 0) on no-match.
// PHILOSOPHY I rejects the `-1` magic value pattern of `search`.
function __taida_str_search_lax(s, rx) {
  if (typeof s !== 'string') return Lax(null, 0);
  if (!__taida_is_regex(rx)) {
    const err = new Error(
      'str.searchLax(...) requires a Regex argument. Use Regex("pattern") to construct one.'
    );
    err.__taida_error_type = 'TypeError';
    throw err;
  }
  const re = __taida_compile_regex(rx, false);
  const m = s.match(re);
  if (!m) return Lax(null, 0);
  const prefix = s.slice(0, m.index);
  return Lax(Array.from(prefix).length);
}
function Slice(val, optsOrStart, maybeEnd) {
  // C25B-031: support both forms —
  //   named:      Slice[val]({start, end})         → optsOrStart is an object
  //   positional: Slice[val, start, end]()         → optsOrStart is an Int
  // This matches the interpreter, which prefers positional type_args over
  // named fields.
  let start = 0;
  let endOpt = undefined;
  if (__taida_isIntNumber(optsOrStart)) {
    start = optsOrStart;
    if (__taida_isIntNumber(maybeEnd)) {
      endOpt = maybeEnd;
    }
  } else if (optsOrStart && typeof optsOrStart === 'object') {
    if (__taida_isIntNumber(optsOrStart.start)) start = optsOrStart.start;
    if (__taida_isIntNumber(optsOrStart.end)) endOpt = optsOrStart.end;
  }
  if (typeof val === 'string') {
    const end = (endOpt !== undefined) ? endOpt : val.length;
    return val.slice(start, end);
  }
  if (val instanceof Uint8Array) {
    const end = (endOpt !== undefined) ? endOpt : val.length;
    const s = Math.max(0, Math.min(val.length, start));
    const e = Math.max(0, Math.min(val.length, end));
    const from = Math.min(s, e);
    const to = Math.max(s, e);
    // D29B-004 / Track-ε: subarray is a zero-copy view sharing the
    // underlying ArrayBuffer (vs. .slice() which deep-copies). Matches
    // the interpreter's Value::bytes_view (Arc<BytesValue> sub-range
    // view sharing buf Arc).
    return val.subarray(from, to);
  }
  return '';
}
function CharAt(str, idx) { return typeof str === 'string' && idx >= 0 && idx < str.length ? Lax(str[idx]) : Lax(null, ''); }
function Repeat(str, n) { return typeof str === 'string' ? str.repeat(Math.max(0, n)) : ''; }
// ── C26B-018 (B) byte-level primitives (UTF-8 byte view) ──
// These operate on the raw UTF-8 byte stream, not on JS UTF-16 code
// units. The TextEncoder round-trip gives O(n) the first time but
// V8 caches the result so repeated calls stay fast. Existing
// `CharAt` / `Slice` / `.length()` are **unchanged** (UTF-16 surface).
const __taida_enc = (typeof TextEncoder !== 'undefined') ? new TextEncoder() : null;
const __taida_dec = (typeof TextDecoder !== 'undefined') ? new TextDecoder('utf-8', { fatal: false }) : null;
function __taida_bytes_of(s) {
  if (typeof s !== 'string') return new Uint8Array(0);
  if (__taida_enc) return __taida_enc.encode(s);
  // Fallback: rough 1-byte-per-char mapping for ASCII-only envs.
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i) & 0xff;
  return out;
}
function ByteAt(str, idx) {
  const bytes = __taida_bytes_of(str);
  if (idx < 0 || idx >= bytes.length) return Lax(null, 0);
  return Lax(bytes[idx] | 0);
}
function ByteSlice(str, start, end) {
  const bytes = __taida_bytes_of(str);
  const len = bytes.length;
  let s = Math.max(0, Math.min(len, start | 0));
  let e = Math.max(0, Math.min(len, end | 0));
  if (s > e) { const t = s; s = e; e = t; }
  const slice = bytes.subarray(s, e);
  if (__taida_dec) return __taida_dec.decode(slice);
  // Fallback
  let out = '';
  for (let i = 0; i < slice.length; i++) out += String.fromCharCode(slice[i]);
  return out;
}
function ByteLength(str) {
  return __taida_bytes_of(str).length;
}
// ── C26B-018 (C) StringRepeatJoin ────────────────────────────
// `StringRepeatJoin[str, n, sep]() -> Str` — single-allocation
// repeat+join. n<=0 → "", n==1 → str (no sep), n>=2 → str+sep+...+str.
function StringRepeatJoin(str, n, sep) {
  if (typeof str !== 'string') str = '';
  if (typeof sep !== 'string') sep = '';
  n = n | 0;
  if (n <= 0) return '';
  if (n === 1) return str;
  // String#repeat + join: V8 optimizes this into a single buffer.
  if (sep === '') return str.repeat(n);
  return new Array(n).fill(str).join(sep);
}
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
// C25B-025 (Phase 5-A): math molds. All return Number; interpreter
// widens Int inputs to f64 first so these accept either. Matches the
// interpreter's `f64::sqrt` etc. semantics (NaN / ±Infinity preserved).
function Sqrt(num) { return typeof num === 'number' ? Math.sqrt(num) : 0; }
function Pow(base, exp) {
  return (typeof base === 'number' && typeof exp === 'number') ? Math.pow(base, exp) : 0;
}
function Exp(num) { return typeof num === 'number' ? Math.exp(num) : 0; }
function Ln(num) { return typeof num === 'number' ? Math.log(num) : 0; }
function Log2(num) { return typeof num === 'number' ? Math.log2(num) : 0; }
function Log10(num) { return typeof num === 'number' ? Math.log10(num) : 0; }
function Log(value, base) {
  if (typeof value !== 'number') return 0;
  if (base === undefined) return Math.log(value);
  if (typeof base !== 'number') return 0;
  return Math.log(value) / Math.log(base);
}
function Sin(num) { return typeof num === 'number' ? Math.sin(num) : 0; }
function Cos(num) { return typeof num === 'number' ? Math.cos(num) : 0; }
function Tan(num) { return typeof num === 'number' ? Math.tan(num) : 0; }
function Asin(num) { return typeof num === 'number' ? Math.asin(num) : 0; }
function Acos(num) { return typeof num === 'number' ? Math.acos(num) : 0; }
function Atan(num) { return typeof num === 'number' ? Math.atan(num) : 0; }
function Atan2(y, x) {
  return (typeof y === 'number' && typeof x === 'number') ? Math.atan2(y, x) : 0;
}
function Sinh(num) { return typeof num === 'number' ? Math.sinh(num) : 0; }
function Cosh(num) { return typeof num === 'number' ? Math.cosh(num) : 0; }
function Tanh(num) { return typeof num === 'number' ? Math.tanh(num) : 0; }

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
  // E34B-021: `desc` is treated as an alias for `reverse` so that
  // every backend honours `Sort[xs](by <= ..., desc <= true)` the
  // same way (Native / WASM already OR these two options together
  // in `lower_molds.rs`).
  if (opts && (opts.reverse || opts.desc)) copy.reverse();
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
// E32B-022 (Lock-N): Lax[Int]-returning replacement for the legacy
// `-1`-sentinel `FindIndex`. Predicate semantics are identical; only
// the return shape (Lax[Int] vs raw `-1` Int) differs.
function FindIndexLax(list, fn) {
  for (let i = 0; i < (list || []).length; i++) {
    if (fn(list[i]) === true) return Lax(i);
  }
  return Lax(null, 0);
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
      rendered = __taida_display_string(arg);
    } else if (arg && arg.__type === 'Lax') {
      // Match interpreter BuchiPack display format.
      // C21B-seed-04 re-fix: when the Lax was produced by `Float_mold_f`
      // (i.e. a Taida `Float[...]()` call), its __value / __default are
      // Float-semantic even if they round to integer JS Numbers. Render
      // them via __taida_float_render so `Lax[3.0]` prints with `.0`.
      const _lhv = arg.has_value;
      const _fmt = arg.__floatHint === true
        ? (n => typeof n === 'number' ? __taida_float_render(n) : __taida_format(n))
        : __taida_format;
      rendered = '@(has_value <= ' + String(!!_lhv) + ', __value <= ' + _fmt(arg.__value) + ', __default <= ' + _fmt(arg.__default) + ', __type <= "Lax")';
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
  // C21B-seed-04 re-fix: nested Float-hinted Lax — render __value/__default
  // as Float (with `.0`) so `@[Float[3.0]()]` etc. matches the interpreter.
  if (v && v.__type === 'Lax' && v.__floatHint === true) {
    const _lhv = v.has_value;
    const _fmt = n => typeof n === 'number' ? __taida_float_render(n) : __taida_format(n);
    return '@(has_value <= ' + String(!!_lhv) + ', __value <= ' + _fmt(v.__value) + ', __default <= ' + _fmt(v.__default) + ', __type <= "Lax")';
  }
  // C23B-003 reopen 2: nested typed runtime objects (HashMap / Set /
  // Gorillax / RelaxedGorillax / TODO / Stream / Molten / Lax / Result
  // / Async) must recurse through `__taida_display_string` so nested
  // HashMap/Set expand to their synthetic full-form pack shape instead
  // of falling through to `String(v)` which invokes the prototype
  // `.toString()` short-form (`HashMap({...})` / `Set({...})`) and
  // breaks 4-backend parity. This mirrors the interpreter's
  // `Value::to_debug_string()` on BuchiPack which recursively calls
  // `to_display_string()` on each field value.
  if (v && typeof v === 'object' && v.__type) {
    return __taida_display_string(v);
  }
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

// C20-2: readline/promises is loaded alongside node:fs so that
// `stdinLine(prompt) ]=> line` has a UTF-8-aware editor available on the
// JS backend. We deliberately choose `readline/promises` over `readline`
// because `rl.question(prompt)` returns a Promise directly — Taida's
// Async[Lax[Str]] surface wraps it without a sync / async bridge.
// (Atomics.wait + SharedArrayBuffer is out of scope per C20_DESIGN.md
// Stop Conditions.)
const __taida_readline_promises_mod = await import('node:readline/promises').catch(() => null);

function __taida_stdin(prompt) {
  if (typeof globalThis.process !== 'undefined' && __taida_fs) {
    // C20-3 (ROOT-14): prompt may arrive as any Value (Int/Bool/BuchiPack);
    // the interpreter and Native convert it via display-string while the JS
    // backend used to hand the raw value to `process.stdout.write`, raising
    // ERR_INVALID_ARG_TYPE outside the try/catch. Stringify explicitly and
    // keep the write inside try so the empty-string fallback is reachable.
    try {
      if (prompt !== undefined && prompt !== null && prompt !== '') {
        process.stdout.write(String(prompt));
      }
      // C20-3 (ROOT-10): previously read one byte at a time and
      // decoded each via `Buffer.toString('utf-8', 0, 1)`. Continuation
      // bytes of a multibyte codepoint decoded in isolation surfaced as
      // U+FFFD, corrupting non-ASCII input. Read into a 4 KiB chunk
      // buffer and let a streaming UTF-8 decoder stitch codepoints
      // across byte-boundaries.
      const decoder = new TextDecoder('utf-8', { fatal: false, ignoreBOM: true });
      const chunk = Buffer.alloc(4096);
      const fd = process.stdin.fd ?? 0;
      let line = '';
      while (true) {
        const n = __taida_fs.readSync(fd, chunk, 0, chunk.length);
        if (n <= 0) break;
        const decoded = decoder.decode(chunk.subarray(0, n), { stream: true });
        const nl = decoded.indexOf('\n');
        if (nl >= 0) {
          line += decoded.substring(0, nl);
          break;
        }
        line += decoded;
      }
      // Flush any residual state held by the streaming decoder so the
      // caller sees a complete line even if the final read ended mid-
      // codepoint (malformed UTF-8 becomes U+FFFD per spec).
      line += decoder.decode();
      return line.replace(/\r$/, '');
    } catch (_e) { return ''; }
  }
  return '';
}

// ── stdinLine — UTF-8-aware line editor (prelude) ─────────
//
// C20-2 (ROOT-7): `stdin` above delegates to the kernel's cooked-mode
// line discipline, which deletes one byte per Backspace. Multibyte
// (日本語 / 한국어 / 中文 / emoji) input therefore becomes corrupted when
// users edit their typing. `stdinLine` routes through
// `node:readline/promises`, whose `rl.question(prompt)` resolves with a
// full UTF-8 line and whose terminal mode understands char-wide
// Backspace / arrow keys / Ctrl-U on real TTYs.
//
// Shape: Async[Lax[Str]]. The Async wrapper is shared across 3 backends
// (Interpreter, JS, Native) — JS is async-only, so we pin the surface
// type to Async and let the Interpreter / Native backends resolve
// immediately. Taida callers write:
//
//     stdinLine("name: ") ]=> line
//     stdout("hi, " + line.getOrDefault("stranger"))
//
// Any failure (missing readline module, EOF on pipe, Ctrl-C, non-TTY
// stdin, …) collapses to `Lax(null, '')` so the default-value guarantee
// is preserved.
async function __taida_stdinLine_inner(prompt) {
  if (!__taida_readline_promises_mod || typeof process === 'undefined') {
    return Lax(null, '');
  }
  // Parity with Interpreter (rustyline) / Native (termios editor):
  // when stdin or stdout is NOT a TTY, we cannot do char-wide editing
  // anyway, so run with `terminal: false`. That stops node:readline
  // from echoing every byte back and emitting CSI 1G/0J cursor
  // sequences to stdout — behaviour that the other two backends do
  // not exhibit because their raw-mode paths short-circuit on pipe.
  const stdinIsTty = !!(process.stdin && process.stdin.isTTY);
  const stdoutIsTty = !!(process.stdout && process.stdout.isTTY);
  const terminalMode = stdinIsTty && stdoutIsTty;

  let rl;
  try {
    rl = __taida_readline_promises_mod.createInterface({
      input: process.stdin,
      output: process.stdout,
      terminal: terminalMode, // TTY: unicode-aware editing. Pipe: line mode.
    });
  } catch (_e) {
    return Lax(null, '');
  }
  try {
    // Stringify prompt (ROOT-14 parity): Interpreter / Native both
    // display-stringify non-Str prompts, so JS must do the same.
    const promptStr = prompt === undefined || prompt === null ? '' : String(prompt);
    // In non-terminal mode readline still accepts the prompt arg and
    // writes it — but only to the output stream (stdout) when there
    // is something to write. To match the Interpreter / Native
    // behaviour (prompt is echoed only on a TTY) we skip the write
    // ourselves when stdout is piped.
    let line;
    // Race rl.question against the 'close' event so that a piped / EOF
    // stdin does NOT hang the process forever. `rl.question(...)` returns
    // a Promise that resolves on a newline; when stdin closes before a
    // newline it never settles (Node 20 behaviour). We pair it with a
    // manual 'close' listener that resolves with a sentinel and convert
    // that sentinel into the Lax failure shape.
    const LAX_EOF = Symbol('stdinLine.eof');
    const questionArg = terminalMode ? promptStr : '';
    if (!terminalMode && promptStr && stdoutIsTty) {
      // Only write the prompt when stdout is a TTY; callers piping
      // stdout (test harnesses, log capture) do not want the prompt
      // interleaved with program output.
      try { process.stdout.write(promptStr); } catch (_e) { /* ignore */ }
    } else if (!terminalMode && promptStr) {
      // stdout is piped — write prompt to stderr so interactive users
      // still see it, matching the interpreter cooked-mode print path.
      try { process.stderr.write(promptStr); } catch (_e) { /* ignore */ }
    }
    const questionPromise = rl.question(questionArg);
    const closePromise = new Promise((resolve) => {
      rl.once('close', () => resolve(LAX_EOF));
    });
    line = await Promise.race([questionPromise, closePromise]);
    if (line === LAX_EOF) return Lax(null, '');
    return Lax(line);
  } catch (_e) {
    return Lax(null, '');
  } finally {
    try { rl.close(); } catch (_e) { /* best-effort close */ }
  }
}

function __taida_stdinLine(prompt) {
  return __taida_async_pending_from_promise(__taida_stdinLine_inner(prompt));
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
      const hv = v.has_value;
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
    // TODO mold unmold: return unm channel (fallback __default, then __value).
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
      const hv = v.has_value;
      if (hv) return v.__value;
      if (typeof process !== 'undefined') process.exit(1);
      throw new __NativeError('><');
    }
    // RelaxedGorillax unmold: success → value, failure → throw (catchable)
    if (v.__type === 'RelaxedGorillax') {
      const hv = v.has_value;
      if (hv) return v.__value;
      const info = __taida_error_info(v.__error);
      throw __taida_error_pack(
        'RelaxedGorillaEscaped',
        info.message ? 'Relaxed gorilla escaped: ' + info.message : 'Relaxed gorilla escaped',
        info.kind || 'RelaxedGorillaEscaped',
        info.code || 0
      );
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
  // E32B-022 (Lock-N): Lax[Int]-returning siblings of indexOf /
  // lastIndexOf. PHILOSOPHY I forbids `-1` magic-value sentinels;
  // callers should use `]=>` / `<=[` / `getOrDefault(...)` off the
  // returned Lax. Both paths use structural equality just like the
  // `-1`-sentinel siblings above.
  Object.defineProperty(Array.prototype, 'indexOfLax', {
    value: function(v) {
      for (let i = 0; i < this.length; i++) {
        if (__taida_equals(this[i], v)) return Lax(i);
      }
      return Lax(null, 0);
    }, enumerable: false, configurable: true
  });
  Object.defineProperty(Array.prototype, 'lastIndexOfLax', {
    value: function(v) {
      for (let i = this.length - 1; i >= 0; i--) {
        if (__taida_equals(this[i], v)) return Lax(i);
      }
      return Lax(null, 0);
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
        if (customDefault !== undefined) {
          return Lax(val, customDefault);
        }
        return Lax(val);
      }
      const def = customDefault !== undefined ? customDefault : (this.length > 0 ? __taida_lax_default(this[0]) : 0);
      return Lax(null, def);
    }, enumerable: false
  });
  // isEmpty() — check if list is empty
  Object.defineProperty(Array.prototype, 'isEmpty', {
    value: function() { return this.length === 0; }, enumerable: false
  });
  // max() — return Lax (empty list returns Lax with has_value=false)
  Object.defineProperty(Array.prototype, 'max', {
    value: function() {
      if (this.length === 0) return Lax(null);
      return Lax(this.reduce((a, b) => a > b ? a : b));
    }, enumerable: false
  });
  // min() — return Lax (empty list returns Lax with has_value=false)
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
      return Lax(null, 0);
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
  // E32B-022 (Lock-N): String.indexOfLax / .lastIndexOfLax return
  // Lax[Int] (PHILOSOPHY I — no `-1` magic). The returned index is
  // char-based to match the interpreter / native runtime; JS's native
  // .indexOf returns a code-unit offset (UTF-16) so we convert via
  // Array.from(prefix).length, matching __taida_str_search.
  Object.defineProperty(String.prototype, 'indexOfLax', {
    value: function(sub) {
      const target = String(sub);
      const codeUnitIdx = this.indexOf(target);
      if (codeUnitIdx < 0) return Lax(null, 0);
      const prefix = this.slice(0, codeUnitIdx);
      return Lax(Array.from(prefix).length);
    }, enumerable: false, configurable: true
  });
  Object.defineProperty(String.prototype, 'lastIndexOfLax', {
    value: function(sub) {
      const target = String(sub);
      const codeUnitIdx = this.lastIndexOf(target);
      if (codeUnitIdx < 0) return Lax(null, 0);
      const prefix = this.slice(0, codeUnitIdx);
      return Lax(Array.from(prefix).length);
    }, enumerable: false, configurable: true
  });
  // get(index) — return Lax for string character access
  Object.defineProperty(String.prototype, 'get', {
    value: function(idx) {
      if (idx >= 0 && idx < this.length) return Lax(this[idx]);
      return Lax(null, '');
    }, enumerable: false
  });
}

// ── Helper: sort object keys for deterministic JSON output ──
//
// C25B-028: monadic packs (Lax / Gorillax / RelaxedGorillax / Result) must
// match the interpreter's `jsonEncode` output, which renders the `__*`
// fields verbatim with two normalizations:
//   - `has_value` is exposed as a real Bool.
//   - `__error` / `__predicate` / `throw` fields hold internal error
//     callables or null; the interpreter represents the absent-error case
//     as `Value::Unit` which serializes as `{}`. We normalise `null` and
//     functions to `{}` here to match.
//   - `__default` is passed through when present (Lax).
function __taida_is_monadic_pack_obj(obj) {
  return obj && typeof obj === 'object' &&
    (obj.__type === 'Lax' || obj.__type === 'Gorillax' ||
     obj.__type === 'RelaxedGorillax' || obj.__type === 'Result');
}
function __taida_normalise_monadic_field(k, v) {
  if (k === 'has_value') {
    return !!v;
  }
  // Absent-error sentinels (null / function / undefined) render as `{}` to
  // match the interpreter's `Value::Unit` → `serde_json` empty object.
  if ((k === '__error' || k === '__predicate' || k === 'throw') &&
      (v === null || v === undefined || typeof v === 'function')) {
    return Object.freeze({});
  }
  return v;
}
function __taidaSortKeys(obj) {
  // C18-2: Pass Enum wrappers through untouched so JSON.stringify invokes
  // their `toJSON` method and emits the variant-name Str.
  if (__taida_isEnumVal(obj)) return obj;
  if (Array.isArray(obj)) return obj.map(__taidaSortKeys);
  if (obj && typeof obj === 'object' && !(obj instanceof __TaidaJSON)) {
    const isMonadic = __taida_is_monadic_pack_obj(obj);
    const sorted = {};
    for (const k of Object.keys(obj).sort()) {
      // Skip __type — internal metadata, not user data
      if (k === '__type') continue;
      if (obj.__type === 'Lax' && k === '__error') continue;
      let v = obj[k];
      // Skip any remaining function-valued fields outside the monadic
      // carve-out — JSON.stringify already drops them, but being explicit
      // here keeps the key-order deterministic.
      if (typeof v === 'function') continue;
      if (isMonadic) v = __taida_normalise_monadic_field(k, v);
      sorted[k] = __taidaSortKeys(v);
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
      // C23B-009 (2026-04-22): interpreter (`src/interpreter/methods.rs:761-783`)
      // emits `@(key <= …, value <= …)` pairs and
      // `docs/reference/standard_library.md:238` documents the return
      // shape as `@[@(key, value)]`. Previously this runtime emitted
      // `{ first, second }` (legacy `zip()`-style convention), which
      // produced `@(first <= "a", second <= 1)` under `Str[m.entries()]()`
      // and diverged from interpreter / Native / WASM (post-fix) /
      // documented API. Fix: use `key` / `value` field names. Note the
      // `hashMap(entries)` constructor at line ~2600 still accepts
      // `.first`/`.second` inputs for back-compat with user code that
      // built its own pair list — the fix is only to the output shape.
      return Object.freeze(_entries.map(e => Object.freeze({ key: e.key, value: e.value })));
    },
    size() {
      return _entries.length;
    },
    isEmpty() {
      return _entries.length === 0;
    },
    merge(other) {
      if (!other || other.__type !== 'HashMap') return hm;
      // C23B-008 reopen (2026-04-22): interpreter semantics (see
      // src/interpreter/methods.rs:787-822) are retain-then-push — any
      // overlap key is removed from self and re-appended at other's
      // position with other's value. The previous implementation did
      // update-in-place (`merged[idx] = oe`), which preserved self's
      // ordinal. Repro: `a=[a,b]`, `b=[c,b,d]`; interpreter → `[a,c,b,d]`,
      // broken backends → `[a,b,c,d]`. Fix: keep only self entries whose
      // key is absent from other (self-order), then append every other
      // entry (other-order).
      const otherKeys = other.__entries.map(e => e.key);
      const kept = _entries.filter(e => !otherKeys.some(k => __taida_equals(e.key, k)));
      const merged = kept.concat(other.__entries);
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

function __taida_typename(x) {
  if (__taida_isEnumVal(x)) {
    const variants = __taida_enumDefs[x.__taida_enum_name];
    const ordinal = x.__taida_enum_ordinal;
    if (variants && ordinal >= 0 && ordinal < variants.length) return variants[ordinal];
    return x.__taida_enum_name;
  }
  if (x && typeof x === 'object' && typeof x.__type === 'string') return x.__type;
  if (x instanceof __TaidaError || x instanceof globalThis.Error) return x.type || x.name || 'Error';
  if (Array.isArray(x) || __taida_isBytes(x) || x instanceof __TaidaJSON || x instanceof __TaidaAsync) return __taida_typeof(x);
  if (x && typeof x === 'object') return '';
  return __taida_typeof(x);
}

// ── JS interop helpers (Molten operations) ──
function __taida_js_spread(target, source) {
  if (Array.isArray(target)) {
    return [...target, ...(Array.isArray(source) ? source : [source])];
  }
  return {...target, ...source};
}

function __taida_js_path_array(path) {
  if (path === null || path === undefined) return [];
  if (Array.isArray(path)) return Array.from(path);
  return [path];
}

function __taida_js_args_array(args) {
  if (args === null || args === undefined) return [];
  if (Array.isArray(args)) return Array.from(args);
  return [args];
}

function __taida_to_js_value(value) {
  if (value === null || value === undefined) return value;
  if (__taida_isEnumVal(value)) return Number(value);
  if (__taida_isBytes(value)) return value;
  if (Array.isArray(value)) return value.map(__taida_to_js_value);
  if (value instanceof __TaidaJSON) return value.__value;
  if (value && typeof value === 'object') {
    if (value.__type === 'Lax') {
      const hv = value.has_value;
      return __taida_to_js_value(hv ? value.__value : value.__default);
    }
    if (value.__type === 'Result') {
      return __taida_to_js_value(value.__value);
    }
    if (value.__type === 'Gorillax' || value.__type === 'RelaxedGorillax') {
      const hv = value.has_value;
      return hv ? __taida_to_js_value(value.__value) : undefined;
    }
    const out = {};
    for (const [key, item] of Object.entries(value)) {
      if (key.startsWith('__') || typeof item === 'function') continue;
      out[key] = __taida_to_js_value(item);
    }
    return out;
  }
  return value;
}

function __taida_from_js_value(value) {
  return value;
}

function __taida_js_key(part) {
  if (typeof part === 'string' || typeof part === 'number' || typeof part === 'symbol') {
    return part;
  }
  return String(part);
}

function __taida_js_get_path(subject, path) {
  let current = subject;
  for (const part of __taida_js_path_array(path)) {
    if (current === null || current === undefined) {
      throw new __TaidaError('JSError', 'JS path cannot traverse nullish value', {});
    }
    current = current[__taida_js_key(part)];
  }
  return current;
}

function __taida_js_parent_and_key(subject, path, op) {
  const parts = __taida_js_path_array(path);
  if (parts.length === 0) {
    throw new __TaidaError('JSError', `${op} requires a non-empty path`, {});
  }
  const key = __taida_js_key(parts[parts.length - 1]);
  const parentPath = parts.slice(0, -1);
  const parent = __taida_js_get_path(subject, parentPath);
  if (parent === null || parent === undefined) {
    throw new __TaidaError('JSError', `${op} target parent is nullish`, {});
  }
  return [parent, key];
}

function __taida_js_get_runner(path) {
  const runnerPath = __taida_js_path_array(path);
  return __taida_cagerilla_runner(function(subject) {
    return __taida_js_get_path(subject, runnerPath);
  });
}

function __taida_js_call_runner(path, args) {
  const runnerPath = __taida_js_path_array(path);
  const runnerArgs = __taida_js_args_array(args).map(__taida_to_js_value);
  return __taida_cagerilla_runner(function(subject) {
    if (runnerPath.length === 0) {
      if (typeof subject !== 'function') {
        throw new __TaidaError('JSError', 'JSCall target is not callable', {});
      }
      return __taida_from_js_value(subject(...runnerArgs));
    }
    const [receiver, key] = __taida_js_parent_and_key(subject, runnerPath, 'JSCall');
    const fn = receiver[key];
    if (typeof fn !== 'function') {
      throw new __TaidaError('JSError', 'JSCall target is not callable', {});
    }
    return __taida_from_js_value(fn.apply(receiver, runnerArgs));
  });
}

function __taida_js_new_runner(path, args) {
  const runnerPath = __taida_js_path_array(path);
  const runnerArgs = __taida_js_args_array(args).map(__taida_to_js_value);
  return __taida_cagerilla_runner(function(subject) {
    const ctor = __taida_js_get_path(subject, runnerPath);
    if (typeof ctor !== 'function') {
      throw new __TaidaError('JSError', 'JSNew target is not constructible', {});
    }
    return __taida_from_js_value(new ctor(...runnerArgs));
  });
}

function __taida_js_set_runner(path, value) {
  const runnerPath = __taida_js_path_array(path);
  const runnerValue = __taida_to_js_value(value);
  return __taida_cagerilla_runner(function(subject) {
    const [receiver, key] = __taida_js_parent_and_key(subject, runnerPath, 'JSSet');
    receiver[key] = runnerValue;
    return subject;
  });
}

function __taida_js_bind_runner(path) {
  const runnerPath = __taida_js_path_array(path);
  return __taida_cagerilla_runner(function(subject) {
    if (runnerPath.length === 0) {
      if (typeof subject !== 'function') {
        throw new __TaidaError('JSError', 'JSBind target is not callable', {});
      }
      return subject.bind(null);
    }
    const [receiver, key] = __taida_js_parent_and_key(subject, runnerPath, 'JSBind');
    const fn = receiver[key];
    if (typeof fn !== 'function') {
      throw new __TaidaError('JSError', 'JSBind target is not callable', {});
    }
    return fn.bind(receiver);
  });
}

function __taida_js_spread_runner(source) {
  const runnerSource = __taida_to_js_value(source);
  return __taida_cagerilla_runner(function(subject) {
    return __taida_from_js_value(__taida_js_spread(subject, runnerSource));
  });
}

// ── taida-lang/os — Core-bundled OS package (13 APIs) ──
// Uses Node.js fs, child_process, process modules.

// ESM: reuse __taida_fs for fs operations, load child_process via dynamic import
const __os_fs = __taida_fs || null;
const __os_cp = await import('node:child_process').catch(() => null);

const __OS_MAX_READ_SIZE = 64 * 1024 * 1024; // 64 MB

// Helper: create os Result success value
function __taida_os_result_ok(inner) {
  return __taida_result_create(inner, null, null, 'os');
}

// Helper: build IoError value from runtime error object.
//
// C19 compatibility: `code` and `kind` are mirrored at the top level so
// that `err.code` / `err.kind` work on the JS backend — matching the
// interpreter's `Value::Error` dot-access behaviour. The `fields` object
// is kept so existing `err.fields.code` callers keep working.
//
// C19B-001: Node exposes `err.errno` as a negative POSIX errno on Linux
// (e.g. `-2` for ENOENT). The interpreter and native backends surface the
// positive errno (`2`), so normalize here so 3-backend parity callers can
// compare `r.__error.code` without per-backend abs().
function __taida_os_io_error(err) {
  let code = err && err.errno !== undefined ? err.errno : -1;
  if (typeof code === 'number' && code < 0 && code !== -1) {
    code = -code;
  }
  const message = err && err.message ? err.message : String(err);
  const kind = __taida_os_classify_error_kind(err);
  return {
    __type: 'IoError',
    type: 'IoError',
    message: message,
    code: code,
    kind: kind,
    fields: { code: code, kind: kind },
  };
}

// Helper: classify error kind from errno/message (mirrors Rust classify_io_error_kind)
function __taida_os_classify_error_kind(err) {
  const errno = err && err.errno !== undefined ? err.errno : -1;
  const code = err && err.code ? err.code : '';
  const msg = err && err.message ? err.message : '';
  if (code === 'ETIMEDOUT' || code === 'EAGAIN' || errno === 110 || errno === 60 || errno === 11
      || msg.includes('timed out')) return 'timeout';
  if (code === 'ECONNREFUSED' || errno === 111 || errno === 61) return 'refused';
  if (code === 'ECONNRESET' || errno === 104 || errno === 54) return 'reset';
  if (code === 'ECONNABORTED' || code === 'EPIPE' || code === 'ENOTCONN'
      || errno === 32 || errno === 57 || errno === 107) return 'peer_closed';
  if (code === 'ENOENT' || errno === 2) return 'not_found';
  if (code === 'EINVAL') return 'invalid';
  return 'unknown';
}

// Helper: create os Result failure from error
function __taida_os_result_fail(err) {
  const code = err && err.errno !== undefined ? err.errno : -1;
  const message = err && err.message ? err.message : String(err);
  const kind = __taida_os_classify_error_kind(err);
  const inner = Object.freeze({ ok: false, code: code, message: message, kind: kind });
  return __taida_result_create(inner, __taida_os_io_error(err), null, 'os');
}

// Helper: create os Result failure with explicit kind/message (non-OS errors)
function __taida_os_result_fail_with_kind(kind, message) {
  const inner = Object.freeze({ ok: false, code: -1, message: message, kind: kind });
  // E33B-003 Cat B: lift `code` and `kind` to top-level for `err.X` parity
  // with Interpreter / Native. Keep `fields.X` for legacy callers.
  const errVal = { __type: 'IoError', type: 'IoError', message: message, code: -1, kind: kind, fields: { code: -1, kind: kind } };
  return __taida_result_create(inner, errVal, null, 'os');
}

function __taida_os_gorillax_ok(inner) {
  return Gorillax(inner, null);
}

function __taida_os_gorillax_fail(errVal) {
  return Gorillax(null, errVal);
}

// Helper: standard success inner @(ok=true, code=0, message="")
function __taida_os_ok_inner() {
  return Object.freeze({ ok: true, code: 0, message: '' });
}

// Helper: process result inner @(stdout, stderr, code)
function __taida_os_process_inner(stdout, stderr, code) {
  return Object.freeze({ stdout: stdout, stderr: stderr, code: code });
}

// C19: code-only inner @(code: Int) for runInteractive / execShellInteractive.
// Intentionally omits stdout / stderr because interactive variants do not
// capture them — the child writes directly to the inherited TTY.
function __taida_os_process_inner_code_only(code) {
  return Object.freeze({ code: code });
}

// C19: POSIX signal name -> number. Only the main ones are mapped; unknown
// signals fall back to 0 so that `128 + signal` becomes 128 rather than NaN.
function __taida_os_signal_to_number(sig) {
  const map = {
    SIGHUP: 1, SIGINT: 2, SIGQUIT: 3, SIGILL: 4, SIGTRAP: 5,
    SIGABRT: 6, SIGBUS: 7, SIGFPE: 8, SIGKILL: 9, SIGUSR1: 10,
    SIGSEGV: 11, SIGUSR2: 12, SIGPIPE: 13, SIGALRM: 14, SIGTERM: 15
  };
  return map[sig] || 0;
}

// C19: extract an exit code from a Node `spawnSync` result, following the
// `128 + signal` convention used by the interpreter and Native backends.
function __taida_os_extract_spawn_sync_code(result) {
  if (result.status !== null && result.status !== undefined) return result.status;
  if (result.signal !== null && result.signal !== undefined) {
    return 128 + __taida_os_signal_to_number(result.signal);
  }
  return -1;
}

// ── Input molds (Read, ListDir, Stat, Exists, EnvVar) ──

function __taida_os_read_error(kind) {
  return Lax(null, '', undefined, __taida_error_pack('IoError', 'Read error', kind || 'other', 0));
}

function __taida_os_error_kind(e) {
  const source = (e && e.code) ? e : (e && e.cause && e.cause.code ? e.cause : e);
  const code = source && source.code ? String(source.code) : '';
  if (code === 'ENOENT' || code === 'ENOTDIR') return 'not_found';
  if (code === 'EACCES' || code === 'EPERM') return 'permission';
  if (code === 'EAGAIN' || code === 'EWOULDBLOCK' || code === 'ETIMEDOUT') return 'timeout';
  if (code === 'ECONNREFUSED') return 'refused';
  if (code === 'ECONNRESET') return 'reset';
  if (code === 'EPIPE' || code === 'ENOTCONN' || code === 'ECONNABORTED') return 'peer_closed';
  if (code === 'EINVAL' || code === 'EBADF') return 'invalid';
  return 'other';
}

function __taida_os_http_default_response() {
  return Object.freeze({ status: 0, body: '', headers: Object.freeze({}) });
}

function __taida_os_http_error(kind) {
  return Lax(
    null,
    __taida_os_http_default_response(),
    undefined,
    __taida_error_pack('IoError', 'HttpRequest error', kind || 'other', 0)
  );
}

function __taida_os_http_url_invalid(url) {
  const s = String(url || '');
  return (s.includes('://') && !(s.startsWith('http://') || s.startsWith('https://')))
    || s.includes('\r')
    || s.includes('\n');
}

function __taida_os_str_lax_error(message, kind) {
  return Lax(null, '', undefined, __taida_error_pack('IoError', message, kind || 'other', 0));
}

function __taida_os_bytes_lax_error(message, kind) {
  return __taida_lax_from_bytes(
    new Uint8Array(0),
    false,
    __taida_error_pack('IoError', message, kind || 'other', 0)
  );
}

function __taida_os_udp_recv_default_payload() {
  return Object.freeze({ host: '', port: 0, data: new Uint8Array(0), truncated: false });
}

function __taida_os_udp_recv_error(kind) {
  return Lax(
    null,
    __taida_os_udp_recv_default_payload(),
    undefined,
    __taida_error_pack('IoError', 'UdpRecvFrom error', kind || 'other', 0)
  );
}

function __taida_os_read(path) {
  if (!__os_fs) return __taida_os_read_error('unavailable');
  try {
    const stat = __os_fs.statSync(path);
    if (stat.size > __OS_MAX_READ_SIZE) return __taida_os_read_error('too_large');
    const content = __os_fs.readFileSync(path, 'utf-8');
    return Lax(content);
  } catch (e) {
    return __taida_os_read_error(__taida_os_error_kind(e));
  }
}

function __taida_os_readBytes(path) {
  if (!__os_fs) return __taida_os_readBytes_error('unavailable');
  try {
    const stat = __os_fs.statSync(path);
    if (stat.size > __OS_MAX_READ_SIZE) return __taida_os_readBytes_error('too_large');
    const content = __os_fs.readFileSync(path);
    return __taida_lax_from_bytes(new Uint8Array(content), true);
  } catch (e) {
    return __taida_os_readBytes_error(__taida_os_error_kind(e));
  }
}

function __taida_os_readBytes_error(kind) {
  return __taida_lax_from_bytes(
    new Uint8Array(0),
    false,
    __taida_error_pack('IoError', 'ReadBytes error', kind || 'other', 0)
  );
}

// C26B-020 柱 1: chunked / large-file bytes read.
// Mirrors the Interpreter semantics from os_eval.rs:
//   - negative offset/len   → Lax failure (default Bytes[])
//   - len > 64 MB ceiling   → Lax failure (default Bytes[])
//   - len == 0              → Lax success with empty Bytes
//   - offset >= file size   → Lax success with empty Bytes
//   - offset + len > size   → Lax success with truncated tail
function __taida_os_readBytesAt(path, offset, len) {
  if (!__os_fs) return __taida_os_readBytesAt_error('unavailable');
  const off = typeof offset === 'bigint' ? Number(offset) : (offset | 0);
  const n = typeof len === 'bigint' ? Number(len) : (len | 0);
  if (off < 0 || n < 0) return __taida_os_readBytesAt_error('invalid');
  if (n > __OS_MAX_READ_SIZE) return __taida_os_readBytesAt_error('too_large');
  if (n === 0) return __taida_lax_from_bytes(new Uint8Array(0), true);
  let fd = -1;
  try {
    const buf = Buffer.alloc(n);
    fd = __os_fs.openSync(path, 'r');
    const filled = __os_fs.readSync(fd, buf, 0, n, off);
    __os_fs.closeSync(fd);
    fd = -1;
    const view = filled === n ? new Uint8Array(buf) : new Uint8Array(buf.buffer, buf.byteOffset, filled);
    return __taida_lax_from_bytes(view, true);
  } catch (e) {
    if (fd !== -1) { try { __os_fs.closeSync(fd); } catch (_) {} }
    return __taida_os_readBytesAt_error(__taida_os_error_kind(e));
  }
}

function __taida_os_readBytesAt_error(kind) {
  return __taida_lax_from_bytes(
    new Uint8Array(0),
    false,
    __taida_error_pack('IoError', 'ReadBytesAt error', kind || 'other', 0)
  );
}

function __taida_os_listdir_error(kind) {
  return Lax(
    null,
    Object.freeze([]),
    undefined,
    __taida_error_pack('IoError', 'ListDir error', kind || 'other', 0)
  );
}

function __taida_os_listdir(path) {
  if (!__os_fs) return __taida_os_listdir_error('unavailable');
  try {
    const entries = __os_fs.readdirSync(path).sort();
    return Lax(Object.freeze(entries));
  } catch (e) {
    return __taida_os_listdir_error(__taida_os_error_kind(e));
  }
}

function __taida_os_stat_default() {
  return Object.freeze({ size: 0, modified: '', isDir: false });
}

function __taida_os_stat_error(kind) {
  return Lax(
    null,
    __taida_os_stat_default(),
    undefined,
    __taida_error_pack('IoError', 'Stat error', kind || 'other', 0)
  );
}

function __taida_os_stat(path) {
  if (!__os_fs) return __taida_os_stat_error('unavailable');
  try {
    const stat = __os_fs.statSync(path);
    const modified = new Date(stat.mtimeMs).toISOString().replace(/\.\d{3}Z$/, 'Z');
    const result = Object.freeze({
      size: stat.size,
      modified: modified,
      isDir: stat.isDirectory()
    });
    return Lax(result);
  } catch (e) {
    return __taida_os_stat_error(__taida_os_error_kind(e));
  }
}

// C12B-021: Exists now returns Result[Bool] instead of bare Bool.
// The Result envelope distinguishes "probe succeeded, path not
// present" from "probe failed (e.g. fs module unavailable)". This
// matches the Interpreter / Native contract.
function __taida_os_exists(path) {
  if (!__os_fs) {
    return __taida_os_result_fail(new __NativeError('fs module not available'));
  }
  try {
    const b = __os_fs.existsSync(path);
    return __taida_os_result_ok(b === true);
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_envvar(name) {
  const val = typeof process !== 'undefined' && process.env ? process.env[name] : undefined;
  if (val !== undefined) return Lax(val);
  return Lax(null, '', undefined, __taida_error_pack('IoError', 'EnvVar error', 'not_found', 0));
}

// ── Side-effect functions (writeFile, appendFile, remove, createDir, rename) ──

// C12B-021: the five write/remove/create APIs now return
// Result[Int]. Inner value is the byte count (writeFile /
// writeBytes / appendFile), the number of entries removed
// (remove), or 1/0 for "newly created"/"already existed"
// (createDir). Rationale and parity matrix: see
// .dev/C12_DESIGN.md §C12B-021.
function __taida_os_writeFile(path, content) {
  try {
    // Compute the byte count from the same encoding Node will use
    // when writing a string (utf-8). This lets the returned Int
    // match the Interpreter's `content.len() as i64` on ASCII/UTF-8
    // inputs byte-for-byte.
    const buf = typeof Buffer !== 'undefined'
      ? Buffer.byteLength(content, 'utf8')
      : (typeof content === 'string' ? content.length : 0);
    __os_fs.writeFileSync(path, content);
    return __taida_os_result_ok(buf);
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_writeBytes(path, content) {
  try {
    const payload = __taida_to_bytes_payload(content);
    __os_fs.writeFileSync(path, payload);
    const n = (payload && typeof payload.length === 'number') ? payload.length : 0;
    return __taida_os_result_ok(n);
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_appendFile(path, content) {
  try {
    const buf = typeof Buffer !== 'undefined'
      ? Buffer.byteLength(content, 'utf8')
      : (typeof content === 'string' ? content.length : 0);
    __os_fs.appendFileSync(path, content);
    return __taida_os_result_ok(buf);
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_remove(path) {
  try {
    // Count the entries BEFORE removal so the returned number is
    // deterministic even when the tree traversal itself partially
    // succeeds (rare with rmSync + recursive, but well-defined).
    let count = 0;
    const walk = (p) => {
      count += 1;
      try {
        const st = __os_fs.lstatSync(p);
        if (st.isDirectory()) {
          for (const name of __os_fs.readdirSync(p)) {
            walk(p + '/' + name);
          }
        }
      } catch (_) { /* swallow — stat can race with rm */ }
    };
    try { walk(path); } catch (_) { count = count || 1; }
    __os_fs.rmSync(path, { recursive: true, force: false });
    return __taida_os_result_ok(count);
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_createDir(path) {
  try {
    let already = false;
    try {
      const st = __os_fs.lstatSync(path);
      already = st.isDirectory();
    } catch (_) {
      already = false;
    }
    __os_fs.mkdirSync(path, { recursive: true });
    return __taida_os_result_ok(already ? 0 : 1);
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_rename(from, to) {
  try {
    __os_fs.renameSync(from, to);
    return __taida_os_result_ok(__taida_os_ok_inner());
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

// ── Process functions (run, execShell) ──

// C19 note: ProcessError objects also mirror `code` at the top level (in
// addition to `fields.code`) so that `.code` on the JS backend matches the
// interpreter's `Value::Error` dot-access behaviour.
function __taida_os_process_error(program_or_cmd, code, is_shell) {
  const message = is_shell
    ? 'Shell command exited with code ' + code + ': ' + program_or_cmd
    : "Process '" + program_or_cmd + "' exited with code " + code;
  return {
    __type: 'ProcessError',
    type: 'ProcessError',
    message: message,
    code: code,
    fields: { code: code },
  };
}

function __taida_os_run(program, args) {
  if (!__os_cp) {
    return __taida_os_gorillax_fail(__taida_os_io_error(new __NativeError('child_process not available')));
  }
  try {
    const result = __os_cp.execFileSync(program, args || [], { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] });
    const inner = __taida_os_process_inner(result, '', 0);
    return __taida_os_gorillax_ok(inner);
  } catch (e) {
    if (e.status !== undefined) {
      // Process exited with non-zero code
      const code = e.status !== null ? e.status : -1;
      return __taida_os_gorillax_fail(__taida_os_process_error(program, code, false));
    }
    return __taida_os_gorillax_fail(__taida_os_io_error(e));
  }
}

function __taida_os_execShell(command) {
  if (!__os_cp) {
    return __taida_os_gorillax_fail(__taida_os_io_error(new __NativeError('child_process not available')));
  }
  try {
    const result = __os_cp.execSync(command, { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] });
    const inner = __taida_os_process_inner(result, '', 0);
    return __taida_os_gorillax_ok(inner);
  } catch (e) {
    if (e.status !== undefined) {
      const code = e.status !== null ? e.status : -1;
      return __taida_os_gorillax_fail(__taida_os_process_error(command, code, true));
    }
    return __taida_os_gorillax_fail(__taida_os_io_error(e));
  }
}

// ── C19: Interactive process functions (TTY passthrough) ──
//
// These variants call spawnSync with stdio: 'inherit', which hands the
// parent process's stdin / stdout / stderr file descriptors directly to the
// child. No pipes are created, and nothing is captured; the child can draw
// a TUI (nvim, vim, less, fzf, git commit) and read keystrokes live.
//
// Contract (must match the interpreter reference exactly):
// - Success: Gorillax.ok(Object.freeze({ code: 0 }))
// - Non-zero exit: Gorillax.err(ProcessError{code})
// - Pre-exec failure (ENOENT etc.): Gorillax.err(IoError{code, kind})
// - Signal death: code = 128 + signum (best-effort)
//
// Note: inner shape is { code } only — stdout / stderr are deliberately
// absent to signal that the caller cannot observe child I/O.

function __taida_os_runInteractive(program, args) {
  if (!__os_cp) {
    return __taida_os_gorillax_fail(__taida_os_io_error(new __NativeError('child_process not available')));
  }
  try {
    const result = __os_cp.spawnSync(program, args || [], { stdio: 'inherit' });

    if (result.error) {
      return __taida_os_gorillax_fail(__taida_os_io_error(result.error));
    }

    const code = __taida_os_extract_spawn_sync_code(result);
    const inner = __taida_os_process_inner_code_only(code);
    if (code === 0) {
      return __taida_os_gorillax_ok(inner);
    }
    return __taida_os_gorillax_fail(__taida_os_process_error(program, code, false));
  } catch (e) {
    return __taida_os_gorillax_fail(__taida_os_io_error(e));
  }
}

function __taida_os_execShellInteractive(command) {
  if (!__os_cp) {
    return __taida_os_gorillax_fail(__taida_os_io_error(new __NativeError('child_process not available')));
  }
  try {
    const isWin = typeof process !== 'undefined' && process.platform === 'win32';
    const shellProgram = isWin ? 'cmd' : 'sh';
    const shellArgs = isWin ? ['/C', command] : ['-c', command];
    const result = __os_cp.spawnSync(shellProgram, shellArgs, { stdio: 'inherit' });

    if (result.error) {
      return __taida_os_gorillax_fail(__taida_os_io_error(result.error));
    }

    const code = __taida_os_extract_spawn_sync_code(result);
    const inner = __taida_os_process_inner_code_only(code);
    if (code === 0) {
      return __taida_os_gorillax_ok(inner);
    }
    return __taida_os_gorillax_fail(__taida_os_process_error(command, code, true));
  } catch (e) {
    return __taida_os_gorillax_fail(__taida_os_io_error(e));
  }
}

// ── Query function (allEnv) ──

function __taida_os_allEnv() {
  const entries = [];
  if (typeof process !== 'undefined' && process.env) {
    for (const [key, value] of Object.entries(process.env)) {
      entries.push(Object.freeze({ key: key, value: value || '' }));
    }
  }
  return __taida_createHashMap(entries);
}

function __taida_os_argv() {
  if (typeof process === 'undefined' || !Array.isArray(process.argv)) {
    return Object.freeze([]);
  }
  return Object.freeze(process.argv.slice(2));
}

// ── Phase 2: Async APIs ───────────────────────────────────

// Helper: build HTTP response Lax
function __taida_os_http_response(status, body, headers) {
  const headerObj = {};
  if (headers) {
    for (const [k, v] of headers) {
      headerObj[k.toLowerCase()] = v;
    }
  }
  return Lax(Object.freeze({ status: status, body: body, headers: Object.freeze(headerObj) }));
}

function __taida_os_http_failure() {
  return __taida_os_http_error('other');
}

// ReadAsync[path]() -> Promise<Lax[Str]>
async function __taida_os_readAsync(path) {
  if (!__os_fs) return __taida_os_read_error('unavailable');
  try {
    const fsp = __os_fs.promises || await import('node:fs/promises').then(m => m.default || m).catch(() => null);
    if (!fsp) return __taida_os_read_error('unavailable');
    const stat = await fsp.stat(path);
    if (stat.size > 64 * 1024 * 1024) return __taida_os_read_error('too_large');
    const content = await fsp.readFile(path, 'utf-8');
    return Lax(content);
  } catch (e) {
    return __taida_os_read_error(__taida_os_error_kind(e));
  }
}

// HttpGet[url]() -> Promise<Lax[@(status, body, headers)]>
async function __taida_os_httpGet(url) {
  if (__taida_os_http_url_invalid(url)) return __taida_os_http_error('invalid');
  try {
    const resp = await fetch(url);
    const body = await resp.text();
    const headers = [];
    resp.headers.forEach((v, k) => headers.push([k, v]));
    return __taida_os_http_response(resp.status, body, headers);
  } catch (e) {
    return __taida_os_http_error(__taida_os_error_kind(e));
  }
}

// HttpPost[url, body]() -> Promise<Lax[@(status, body, headers)]>
async function __taida_os_httpPost(url, body) {
  if (__taida_os_http_url_invalid(url)) return __taida_os_http_error('invalid');
  try {
    const resp = await fetch(url, { method: 'POST', body: body || '' });
    const respBody = await resp.text();
    const headers = [];
    resp.headers.forEach((v, k) => headers.push([k, v]));
    return __taida_os_http_response(resp.status, respBody, headers);
  } catch (e) {
    return __taida_os_http_error(__taida_os_error_kind(e));
  }
}

// HttpRequest[method, url](headers, body) -> Promise<Lax[@(status, body, headers)]>
//
// C20-4 (C19B-007): `reqHeaders` now accepts two shapes to mirror the
// interpreter and native backends:
//
//   * BuchiPack object — legacy `headers <= @(content_type <= "...")`
//     (each own-enumerable key is treated as a wire header name; the
//     identifier ban on `-` means dash-bearing headers are unreachable
//     this way).
//   * Array of records — new `headers <= @[@(name <= "x-api-key",
//     value <= "...")]`. Each entry with Str `name` + Str `value`
//     contributes one wire header; arbitrary UTF-8 is allowed in the
//     name, so `x-api-key`, `anthropic-version`, etc. are expressible.
async function __taida_os_httpRequest(method, url, reqHeaders, body) {
  if (__taida_os_http_url_invalid(url) || String(method || '').includes('\r') || String(method || '').includes('\n')) {
    return __taida_os_http_error('invalid');
  }
  try {
    const opts = { method: method || 'GET' };
    if (body) opts.body = body;
    if (reqHeaders) {
      const h = {};
      if (Array.isArray(reqHeaders)) {
        for (const rec of reqHeaders) {
          if (rec && typeof rec === 'object') {
            const n = rec.name;
            const v = rec.value;
            if (typeof n === 'string' && typeof v === 'string' && n.length > 0) {
              h[n] = v;
            }
          }
        }
      } else if (typeof reqHeaders === 'object') {
        for (const [k, v] of Object.entries(reqHeaders)) {
          if (typeof v === 'string') h[k] = v;
        }
      }
      if (Object.keys(h).length > 0) opts.headers = h;
    }
    const resp = await fetch(url, opts);
    const respBody = await resp.text();
    const headers = [];
    resp.headers.forEach((v, k) => headers.push([k, v]));
    return __taida_os_http_response(resp.status, respBody, headers);
  } catch (e) {
    return __taida_os_http_error(__taida_os_error_kind(e));
  }
}

// TCP: load net module
const __os_net = await import('node:net').catch(() => null);
const __os_tls = await import('node:tls').catch(() => null);
const __os_dgram = await import('node:dgram').catch(() => null);
const __TAIDA_OS_NETWORK_TIMEOUT_MS = 30000;

function __taida_os_network_timeout_ms(timeoutMs) {
  if (typeof timeoutMs === 'number' && Number.isFinite(timeoutMs)) {
    const ms = Math.floor(timeoutMs);
    if (ms > 0 && ms <= 600000) return ms;
  }
  return __TAIDA_OS_NETWORK_TIMEOUT_MS;
}

// tcpConnect(host, port, timeoutMs?) -> Promise<Result[@(socket, host, port), _]>
async function __taida_os_tcpConnect(host, port, timeoutMs) {
  if (!__os_net) return __taida_os_result_fail(new __NativeError('net module not available'));
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    const socket = new (__os_net.Socket || __os_net.default.Socket)();
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('connect', onConnect);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onConnect = () => {
      const inner = Object.freeze({ socket: socket, host: host, port: port });
      finish(__taida_os_result_ok(inner));
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`tcpConnect: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      if (typeof socket.destroy === 'function') socket.destroy();
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    socket.once('connect', onConnect);
    socket.once('error', onError);
    try {
      socket.connect(port, host);
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// tcpListen(port, timeoutMs?) -> Promise<Result[@(listener, port), _]>
async function __taida_os_tcpListen(port, timeoutMs) {
  if (!__os_net) return __taida_os_result_fail(new __NativeError('net module not available'));
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    const server = (__os_net.createServer || __os_net.default.createServer)();
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      server.removeListener('listening', onListening);
      server.removeListener('error', onError);
      resolve(result);
    };
    const onListening = () => {
      const inner = Object.freeze({ listener: server, port: port });
      finish(__taida_os_result_ok(inner));
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`tcpListen: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      try { server.close(); } catch (_) {}
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    server.once('listening', onListening);
    server.once('error', onError);
    try {
      server.listen(port, '127.0.0.1');
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// tcpAccept(listener, timeoutMs?) -> Promise<Result[@(socket, host, port), _]>
async function __taida_os_tcpAccept(listenerOrPack, timeoutMs) {
  const server = (listenerOrPack && listenerOrPack.listener) ? listenerOrPack.listener : listenerOrPack;
  if (!server || !server.once) return __taida_os_result_fail(new __NativeError('tcpAccept: invalid listener'));
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      server.removeListener('connection', onConnection);
      server.removeListener('error', onError);
      resolve(result);
    };
    const onConnection = (socket) => {
      const addr = socket.remoteAddress || '';
      const port = socket.remotePort || 0;
      const inner = Object.freeze({ socket: socket, host: addr, port: port });
      finish(__taida_os_result_ok(inner));
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`tcpAccept: timed out after ${effectiveTimeout}ms`);
      try { err.errno = 110; } catch (_) {}
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    server.once('connection', onConnection);
    server.once('error', onError);
  });
}

// socketSend(socket, data, timeoutMs?) -> Promise<Result[@(ok, bytesSent), _]>
async function __taida_os_socketSend(socketOrPack, data, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || !socket.write) return __taida_os_result_fail(new __NativeError('Invalid socket'));
  const payload = __taida_to_bytes_payload(data);
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`socketSend: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    socket.once('error', onError);
    try {
      socket.write(payload, (err) => {
      if (err) {
          finish(__taida_os_result_fail(err));
      } else {
        const inner = Object.freeze({ ok: true, bytesSent: payload.length });
          finish(__taida_os_result_ok(inner));
      }
      });
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// socketSendBytes(socket, data, timeoutMs?) -> Promise<Result[@(ok, bytesSent), _]>
async function __taida_os_socketSendBytes(socketOrPack, data, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || !socket.write) return __taida_os_result_fail(new __NativeError('Invalid socket'));
  const payload = __taida_to_bytes_payload(data);
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`socketSendBytes: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    socket.once('error', onError);
    try {
      socket.write(payload, (err) => {
      if (err) {
          finish(__taida_os_result_fail(err));
      } else {
        const inner = Object.freeze({ ok: true, bytesSent: payload.length });
          finish(__taida_os_result_ok(inner));
      }
      });
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// socketSendAll(socket, data, timeoutMs?) -> Promise<Result[@(ok, bytesSent), _]>
// In Node.js, socket.write() already buffers, so this is equivalent to socketSend.
async function __taida_os_socketSendAll(socketOrPack, data, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || !socket.write) return __taida_os_result_fail(new __NativeError('socketSendAll: invalid socket'));
  const payload = __taida_to_bytes_payload(data);
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`socketSendAll: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    socket.once('error', onError);
    try {
      socket.write(payload, (err) => {
      if (err) {
          finish(__taida_os_result_fail(err));
      } else {
        const inner = Object.freeze({ ok: true, bytesSent: payload.length });
          finish(__taida_os_result_ok(inner));
      }
      });
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// socketRecv(socket, timeoutMs?) -> Promise<Lax[Str]>
async function __taida_os_socketRecv(socketOrPack, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || !socket.once) return __taida_os_str_lax_error('SocketRecv error', 'invalid');
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('data', onData);
      socket.removeListener('end', onEnd);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onData = (chunk) => {
      finish(Lax(chunk.toString('utf-8')));
    };
    const onEnd = () => {
      finish(__taida_os_str_lax_error('SocketRecv error', 'peer_closed'));
    };
    const onError = (err) => {
      finish(__taida_os_str_lax_error('SocketRecv error', __taida_os_error_kind(err)));
    };
    const timer = setTimeout(() => {
      finish(__taida_os_str_lax_error('SocketRecv error', 'timeout'));
    }, effectiveTimeout);

    socket.once('data', onData);
    socket.once('end', onEnd);
    socket.once('error', onError);
  });
}

// socketRecvBytes(socket, timeoutMs?) -> Promise<Lax[Bytes]>
async function __taida_os_socketRecvBytes(socketOrPack, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || !socket.once) return __taida_os_bytes_lax_error('SocketRecvBytes error', 'invalid');
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('data', onData);
      socket.removeListener('end', onEnd);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onData = (chunk) => {
      const bytes = chunk instanceof Uint8Array ? chunk : new Uint8Array(chunk);
      finish(__taida_lax_from_bytes(bytes, true));
    };
    const onEnd = () => {
      finish(__taida_os_bytes_lax_error('SocketRecvBytes error', 'peer_closed'));
    };
    const onError = (err) => {
      finish(__taida_os_bytes_lax_error('SocketRecvBytes error', __taida_os_error_kind(err)));
    };
    const timer = setTimeout(() => {
      finish(__taida_os_bytes_lax_error('SocketRecvBytes error', 'timeout'));
    }, effectiveTimeout);

    socket.once('data', onData);
    socket.once('end', onEnd);
    socket.once('error', onError);
  });
}

// udpBind(host, port, timeoutMs?) -> Promise<Result[@(socket, host, port), _]>
async function __taida_os_udpBind(host, port, timeoutMs) {
  if (!__os_dgram) return __taida_os_result_fail(new __NativeError('dgram module not available'));
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    const socket = (__os_dgram.createSocket || __os_dgram.default.createSocket)('udp4');
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('listening', onListening);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onListening = () => {
      const inner = Object.freeze({ socket: socket, host: host, port: port });
      finish(__taida_os_result_ok(inner));
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`udpBind: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      try { socket.close(); } catch (_) {}
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    socket.once('listening', onListening);
    socket.once('error', onError);
    try {
      socket.bind(port, host);
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// udpSendTo(socket, host, port, data, timeoutMs?) -> Promise<Result[@(ok, bytesSent), _]>
async function __taida_os_udpSendTo(socketOrPack, host, port, data, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || typeof socket.send !== 'function') {
    return __taida_os_result_fail(new __NativeError('udpSendTo: invalid socket'));
  }
  const payload = __taida_to_bytes_payload(data);
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onError = (err) => {
      finish(__taida_os_result_fail(err));
    };
    const timer = setTimeout(() => {
      const err = new __NativeError(`udpSendTo: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);

    socket.once('error', onError);
    try {
      socket.send(payload, port, host, (err, bytes) => {
        if (err) {
          finish(__taida_os_result_fail(err));
        } else {
          const inner = Object.freeze({ ok: true, bytesSent: bytes });
          finish(__taida_os_result_ok(inner));
        }
      });
    } catch (e) {
      finish(__taida_os_result_fail(e));
    }
  });
}

// udpRecvFrom(socket, timeoutMs?) -> Promise<Lax[@(host, port, data, truncated)]>
async function __taida_os_udpRecvFrom(socketOrPack, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  const defaultPayload = __taida_os_udp_recv_default_payload();
  if (!socket || typeof socket.once !== 'function') return __taida_os_udp_recv_error('invalid');
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('message', onMessage);
      socket.removeListener('error', onError);
      resolve(result);
    };
    const onMessage = (msg, rinfo) => {
      const cap = 65507;
      const truncated = msg.length > cap;
      const data = truncated ? msg.subarray(0, cap) : msg;
      const payload = Object.freeze({
        host: (rinfo && typeof rinfo.address === 'string') ? rinfo.address : '',
        port: (rinfo && Number.isFinite(rinfo.port)) ? rinfo.port : 0,
        data: new Uint8Array(data),
        truncated: truncated,
      });
      finish(Lax(payload, defaultPayload));
    };
    const onError = (err) => {
      finish(__taida_os_udp_recv_error(__taida_os_error_kind(err)));
    };
    const timer = setTimeout(() => {
      finish(__taida_os_udp_recv_error('timeout'));
    }, effectiveTimeout);

    socket.once('message', onMessage);
    socket.once('error', onError);
  });
}

// socketClose(socket) -> Promise<Result[@(ok, code, message), _]>
async function __taida_os_socketClose(socketOrPack) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || typeof socket !== 'object') {
    return __taida_os_result_fail(new __NativeError('socketClose: invalid socket'));
  }
  if (socket.__taidaClosed === true || socket.destroyed === true) {
    return __taida_os_result_fail(new __NativeError('socketClose: socket already closed'));
  }
  try {
    if (typeof socket.end === 'function') socket.end();
    if (typeof socket.close === 'function') socket.close();
    if (typeof socket.destroy === 'function') socket.destroy();
    socket.__taidaClosed = true;
    return __taida_os_result_ok(__taida_os_ok_inner());
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

// listenerClose(listener) -> Promise<Result[@(ok, code, message), _]>
async function __taida_os_listenerClose(listenerOrPack) {
  const listener = (listenerOrPack && listenerOrPack.listener) ? listenerOrPack.listener : listenerOrPack;
  if (!listener || typeof listener.close !== 'function') {
    return __taida_os_result_fail(new __NativeError('listenerClose: invalid listener'));
  }
  if (listener.__taidaClosed === true || listener.listening === false) {
    return __taida_os_result_fail(new __NativeError('listenerClose: listener already closed'));
  }

  return new Promise((resolve) => {
    listener.close((err) => {
      if (err) {
        resolve(__taida_os_result_fail(err));
      } else {
        listener.__taidaClosed = true;
        resolve(__taida_os_result_ok(__taida_os_ok_inner()));
      }
    });
  });
}

// udpClose(socket) is an alias of socketClose(socket)
async function __taida_os_udpClose(socketOrPack) {
  return __taida_os_socketClose(socketOrPack);
}

// ── socketRecvExact(socket, size, timeoutMs?) → Promise<Lax[Bytes]> ──
async function __taida_os_socketRecvExact(socketOrPack, size, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || !socket.once) return __taida_os_bytes_lax_error('SocketRecvExact error', 'invalid');
  if (!__taida_isIntNumber(size) || size < 0) return __taida_os_bytes_lax_error('SocketRecvExact error', 'invalid');
  if (size === 0) return __taida_lax_from_bytes(new Uint8Array(0), true);
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const chunks = [];
    let received = 0;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.removeListener('data', onData);
      socket.removeListener('error', onError);
      socket.removeListener('end', onEnd);
      resolve(result);
    };
    const onData = (chunk) => {
      const buf = chunk instanceof Uint8Array ? chunk : Buffer.from(chunk);
      chunks.push(buf);
      received += buf.length;
      if (received >= size) {
        const all = Buffer.concat(chunks);
        const exact = new Uint8Array(all.slice(0, size));
        // Push remaining bytes back (if any) by unshifting
        if (all.length > size) {
          socket.unshift(all.slice(size));
        }
        finish(__taida_lax_from_bytes(exact, true));
      }
    };
    const onError = (err) => finish(__taida_os_bytes_lax_error('SocketRecvExact error', __taida_os_error_kind(err)));
    const onEnd = () => finish(__taida_os_bytes_lax_error('SocketRecvExact error', 'peer_closed'));
    const timer = setTimeout(() => finish(__taida_os_bytes_lax_error('SocketRecvExact error', 'timeout')), effectiveTimeout);
    socket.on('data', onData);
    socket.once('error', onError);
    socket.once('end', onEnd);
  });
}

// ── dnsResolve(host, timeoutMs?) → Promise<Result[@(addresses), _]> ──
async function __taida_os_dnsResolve(host, timeoutMs) {
  const dns = await import('node:dns').catch(() => null);
  if (!dns) return __taida_os_result_fail(new __NativeError('dns module not available'));
  const effectiveTimeout = __taida_os_network_timeout_ms(timeoutMs);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (result) => { if (!settled) { settled = true; clearTimeout(timer); resolve(result); } };
    const timer = setTimeout(() => {
      const err = new __NativeError(`dnsResolve: timed out after ${effectiveTimeout}ms`);
      err.errno = 110;
      finish(__taida_os_result_fail(err));
    }, effectiveTimeout);
    dns.promises.lookup(host, { all: true }).then((results) => {
      const seen = new __NativeSet();
      const addrs = [];
      for (const r of results) {
        if (!seen.has(r.address)) { seen.add(r.address); addrs.push(r.address); }
      }
      if (addrs.length === 0) {
        const err = new __NativeError(`dnsResolve: no records for '${host}'`);
        err.code = 'ENOENT';
        finish(__taida_os_result_fail(err));
      } else {
        finish(__taida_os_result_ok(Object.freeze({ addresses: addrs })));
      }
    }).catch((err) => {
      finish(__taida_os_result_fail(err));
    });
  });
}

// ── Pool management (in-process, no real connections) ──
const __taida_pool_states = new __NativeMap();
let __taida_next_pool_id = 1;

function __taida_os_poolCreate(config) {
  if (!config || typeof config !== 'object') {
    return __taida_os_result_fail_with_kind('invalid', 'poolCreate: config must be a pack, got ' + String(config));
  }
  const maxSize = config.maxSize !== undefined ? config.maxSize : 10;
  if (!__taida_isIntNumber(maxSize) || maxSize <= 0) {
    return __taida_os_result_fail_with_kind('invalid', 'poolCreate: maxSize must be > 0, got ' + maxSize);
  }
  let maxIdle = config.maxIdle !== undefined ? config.maxIdle : maxSize;
  if (!__taida_isIntNumber(maxIdle) || maxIdle < 0) {
    return __taida_os_result_fail_with_kind('invalid', 'poolCreate: maxIdle must be >= 0, got ' + maxIdle);
  }
  if (maxIdle > maxSize) maxIdle = maxSize;
  const acquireTimeoutMs = config.acquireTimeoutMs !== undefined ? config.acquireTimeoutMs : 30000;
  if (!__taida_isIntNumber(acquireTimeoutMs) || acquireTimeoutMs <= 0) {
    return __taida_os_result_fail_with_kind('invalid', 'poolCreate: acquireTimeoutMs must be > 0, got ' + acquireTimeoutMs);
  }
  const poolId = __taida_next_pool_id++;
  __taida_pool_states.set(poolId, {
    open: true, maxSize, maxIdle, acquireTimeoutMs,
    idle: [], inUse: new __NativeSet(), nextToken: 1
  });
  return __taida_os_result_ok(Object.freeze({ pool: poolId }));
}

async function __taida_os_poolAcquire(poolOrPack, timeoutMs) {
  const poolId = (poolOrPack && poolOrPack.pool !== undefined) ? poolOrPack.pool
               : (__taida_isIntNumber(poolOrPack) ? poolOrPack : -1);
  const state = __taida_pool_states.get(poolId);
  if (!state) return __taida_os_result_fail_with_kind('invalid', 'poolAcquire: unknown pool handle');
  if (!state.open) return __taida_os_result_fail_with_kind('closed', 'poolAcquire: pool is closed');
  const effectiveTimeout = (__taida_isIntNumber(timeoutMs) && timeoutMs > 0)
    ? timeoutMs : state.acquireTimeoutMs;
  let resource = Object.freeze({});
  let token;
  if (state.idle.length > 0) {
    const entry = state.idle.pop();
    resource = entry.resource;
    token = entry.token;
  } else if (state.inUse.size < state.maxSize) {
    token = state.nextToken++;
  } else {
    return __taida_os_result_fail_with_kind('timeout', `poolAcquire: timed out after ${effectiveTimeout}ms`);
  }
  state.inUse.add(token);
  return __taida_os_result_ok(Object.freeze({ resource, token }));
}

function __taida_os_poolRelease(poolOrPack, token, resource) {
  const poolId = (poolOrPack && poolOrPack.pool !== undefined) ? poolOrPack.pool
               : (__taida_isIntNumber(poolOrPack) ? poolOrPack : -1);
  const state = __taida_pool_states.get(poolId);
  if (!state) return __taida_os_result_fail_with_kind('invalid', 'poolRelease: unknown pool handle');
  if (!state.open) return __taida_os_result_fail_with_kind('closed', 'poolRelease: pool is closed');
  if (!state.inUse.has(token)) return __taida_os_result_fail_with_kind('invalid', 'poolRelease: token is not in-use');
  state.inUse.delete(token);
  let reused = false;
  if (state.idle.length < state.maxIdle) {
    state.idle.push({ token, resource });
    reused = true;
  }
  return __taida_os_result_ok(Object.freeze({ ok: true, reused }));
}

async function __taida_os_poolClose(poolOrPack) {
  const poolId = (poolOrPack && poolOrPack.pool !== undefined) ? poolOrPack.pool
               : (__taida_isIntNumber(poolOrPack) ? poolOrPack : -1);
  const state = __taida_pool_states.get(poolId);
  if (!state) return __taida_os_result_fail_with_kind('invalid', 'poolClose: unknown pool handle');
  if (!state.open) return __taida_os_result_fail_with_kind('closed', 'poolClose: pool already closed');
  state.open = false;
  state.idle.length = 0;
  state.inUse.clear();
  return __taida_os_result_ok(Object.freeze({ ok: true }));
}

function __taida_os_poolHealth(poolOrPack) {
  const poolId = (poolOrPack && poolOrPack.pool !== undefined) ? poolOrPack.pool
               : (__taida_isIntNumber(poolOrPack) ? poolOrPack : -1);
  const state = __taida_pool_states.get(poolId);
  if (!state) return Object.freeze({ open: false, idle: 0, inUse: 0, waiting: 0 });
  return Object.freeze({
    open: state.open,
    idle: state.idle.length,
    inUse: state.inUse.size,
    waiting: 0
  });
}

// ── Cancel mold: Cancel[async]() → cancelled Async ──
function Cancel_mold(asyncVal) {
  if (asyncVal instanceof __TaidaAsync) {
    if (asyncVal.status === 'pending') {
      return new __TaidaAsync(
        null,
        new __TaidaError('CancelledError', 'Async operation cancelled', {}),
        'rejected'
      );
    }
    return asyncVal;
  }
  // Non-async: wrap as fulfilled
  return new __TaidaAsync(asyncVal, null, 'fulfilled');
}

// ── Endian pack/unpack molds ──
function U16BE_mold(value) {
  if (!__taida_isIntNumber(value) || value < 0 || value > 65535)
    return __taida_lax_from_bytes(new Uint8Array(0), false);
  return __taida_lax_from_bytes(new Uint8Array([(value >> 8) & 0xff, value & 0xff]), true);
}
function U16LE_mold(value) {
  if (!__taida_isIntNumber(value) || value < 0 || value > 65535)
    return __taida_lax_from_bytes(new Uint8Array(0), false);
  return __taida_lax_from_bytes(new Uint8Array([value & 0xff, (value >> 8) & 0xff]), true);
}
function U32BE_mold(value) {
  if (!__taida_isIntNumber(value) || value < 0 || value > 4294967295)
    return __taida_lax_from_bytes(new Uint8Array(0), false);
  return __taida_lax_from_bytes(new Uint8Array([
    (value >>> 24) & 0xff, (value >>> 16) & 0xff, (value >>> 8) & 0xff, value & 0xff
  ]), true);
}
function U32LE_mold(value) {
  if (!__taida_isIntNumber(value) || value < 0 || value > 4294967295)
    return __taida_lax_from_bytes(new Uint8Array(0), false);
  return __taida_lax_from_bytes(new Uint8Array([
    value & 0xff, (value >>> 8) & 0xff, (value >>> 16) & 0xff, (value >>> 24) & 0xff
  ]), true);
}
function U16BEDecode_mold(bytes) {
  if (!(bytes instanceof Uint8Array) || bytes.length !== 2) return Lax(null, 0);
  return Lax((bytes[0] << 8) | bytes[1]);
}
function U16LEDecode_mold(bytes) {
  if (!(bytes instanceof Uint8Array) || bytes.length !== 2) return Lax(null, 0);
  return Lax((bytes[1] << 8) | bytes[0]);
}
function U32BEDecode_mold(bytes) {
  if (!(bytes instanceof Uint8Array) || bytes.length !== 4) return Lax(null, 0);
  return Lax(((bytes[0] << 24) | (bytes[1] << 16) | (bytes[2] << 8) | bytes[3]) >>> 0);
}
function U32LEDecode_mold(bytes) {
  if (!(bytes instanceof Uint8Array) || bytes.length !== 4) return Lax(null, 0);
  return Lax(((bytes[3] << 24) | (bytes[2] << 16) | (bytes[1] << 8) | bytes[0]) >>> 0);
}

// ── BytesCursor molds ──
function BytesCursor_mold(bytesVal) {
  if (!(bytesVal instanceof Uint8Array)) bytesVal = new Uint8Array(0);
  const offset = 0;
  return Object.freeze({
    __type: 'BytesCursor',
    bytes: bytesVal,
    offset: offset,
    length: bytesVal.length
  });
}
function BytesCursorRemaining_mold(cursor) {
  if (!cursor || cursor.__type !== 'BytesCursor') return 0;
  return Math.max(0, cursor.bytes.length - cursor.offset);
}
function BytesCursorTake_mold(cursor, size) {
  const makeCursor = (b, o) => Object.freeze({ __type: 'BytesCursor', bytes: b, offset: o, length: b.length });
  const makeStep = (v, c) => Object.freeze({ value: v, cursor: c });
  if (!cursor || cursor.__type !== 'BytesCursor') {
    const emptyCursor = makeCursor(new Uint8Array(0), 0);
    const defStep = makeStep(new Uint8Array(0), emptyCursor);
    return __taida_lax_from_bytes_cursor_step(defStep, false);
  }
  const bytes = cursor.bytes;
  const off = cursor.offset;
  const currentCursor = makeCursor(bytes, off);
  const defStep = makeStep(new Uint8Array(0), currentCursor);
  if (!__taida_isIntNumber(size) || size < 0) {
    return __taida_lax_from_bytes_cursor_step(defStep, false);
  }
  if (off + size > bytes.length) {
    return __taida_lax_from_bytes_cursor_step(defStep, false);
  }
  const chunk = new Uint8Array(bytes.slice(off, off + size));
  const nextCursor = makeCursor(bytes, off + size);
  const step = makeStep(chunk, nextCursor);
  return __taida_lax_from_bytes_cursor_step(step, true);
}
function BytesCursorU8_mold(cursor) {
  const makeCursor = (b, o) => Object.freeze({ __type: 'BytesCursor', bytes: b, offset: o, length: b.length });
  const makeStep = (v, c) => Object.freeze({ value: v, cursor: c });
  if (!cursor || cursor.__type !== 'BytesCursor') {
    const emptyCursor = makeCursor(new Uint8Array(0), 0);
    const defStep = makeStep(0, emptyCursor);
    return __taida_lax_from_bytes_cursor_step(defStep, false);
  }
  const bytes = cursor.bytes;
  const off = cursor.offset;
  const currentCursor = makeCursor(bytes, off);
  const defStep = makeStep(0, currentCursor);
  if (off >= bytes.length) {
    return __taida_lax_from_bytes_cursor_step(defStep, false);
  }
  const value = bytes[off];
  const nextCursor = makeCursor(bytes, off + 1);
  const step = makeStep(value, nextCursor);
  return __taida_lax_from_bytes_cursor_step(step, true);
}
// Helper: create Lax wrapping a BytesCursor step (value+cursor pair)
function __taida_lax_from_bytes_cursor_step(step, hasValue) {
  const val = step;
  const def = Object.freeze({ value: step.value, cursor: step.cursor });
  return Object.freeze({
    __type: 'Lax',
    __value: hasValue ? val : def,
    __default: def,
    has_value: !!hasValue,
    hasValue() { return !!hasValue; },
    isEmpty() { return !hasValue; },
    errorInfo() { return __taida_error_info_lax(null); },
    getOrDefault(d) { return hasValue ? val : d; },
    map(fn) { return hasValue ? Lax(fn(val)) : this; },
    flatMap(fn) {
      if (!hasValue) return this;
      const result = fn(val);
      if (result && result.__type === 'Lax') return result;
      return Lax(result);
    },
    unmold() { return hasValue ? val : def; },
    toString() { return hasValue ? 'Lax(BytesCursorStep)' : 'Lax(default: BytesCursorStep)'; },
  });
}

// ── taida-lang/crypto: sha256 ──────────────────────────────────
const __taida_crypto = await import('node:crypto').catch(() => null);
function sha256(value) {
  const data = typeof value === 'string' ? value : String(value);
  if (__taida_crypto) {
    return __taida_crypto.createHash('sha256').update(data, 'utf8').digest('hex');
  }
  // Fallback: pure-JS SHA-256 (should not reach here in Node.js)
  return '';
}

// ── taida-lang/net: HTTP v1 runtime ─────────────────────────────

// ── C26B-016 (@c.26, Option B+): span-aware comparison helpers ──
// A span pack is `@(start: Int, len: Int)` — a view over a raw Bytes/Str.
// `raw` can be a Buffer, Uint8Array, or Str. `needle` / `prefix` may be
// Str or Buffer. Invalid inputs return `false` / empty sub-span (tolerant
// hot-path semantics, matching the interpreter).
function __taida_net_spanPackToOffsets(span) {
  if (span && typeof span === 'object' && 'start' in span && 'len' in span) {
    const start = Number(span.start);
    const len = Number(span.len);
    if (Number.isFinite(start) && Number.isFinite(len) && start >= 0 && len >= 0) {
      return [start | 0, len | 0];
    }
  }
  return null;
}
function __taida_net_rawToBuffer(raw) {
  if (Buffer.isBuffer(raw)) { return raw; }
  if (raw instanceof Uint8Array) { return Buffer.from(raw); }
  if (typeof raw === 'string') { return Buffer.from(raw, 'utf8'); }
  return null;
}
function __taida_net_needleToBuffer(needle) {
  if (Buffer.isBuffer(needle)) { return needle; }
  if (needle instanceof Uint8Array) { return Buffer.from(needle); }
  if (typeof needle === 'string') { return Buffer.from(needle, 'utf8'); }
  return null;
}
function __taida_net_SpanEquals(span, raw, needle) {
  const offsets = __taida_net_spanPackToOffsets(span);
  const buf = __taida_net_rawToBuffer(raw);
  const needleBuf = __taida_net_needleToBuffer(needle);
  if (!offsets || !buf || !needleBuf) { return false; }
  const start = offsets[0];
  const len = offsets[1];
  if (start + len > buf.length) { return false; }
  if (len !== needleBuf.length) { return false; }
  for (let i = 0; i < len; i++) {
    if (buf[start + i] !== needleBuf[i]) { return false; }
  }
  return true;
}
function __taida_net_SpanStartsWith(span, raw, prefix) {
  const offsets = __taida_net_spanPackToOffsets(span);
  const buf = __taida_net_rawToBuffer(raw);
  const prefixBuf = __taida_net_needleToBuffer(prefix);
  if (!offsets || !buf || !prefixBuf) { return false; }
  const start = offsets[0];
  const len = offsets[1];
  if (start + len > buf.length) { return false; }
  if (len < prefixBuf.length) { return false; }
  for (let i = 0; i < prefixBuf.length; i++) {
    if (buf[start + i] !== prefixBuf[i]) { return false; }
  }
  return true;
}
function __taida_net_SpanContains(span, raw, needle) {
  const offsets = __taida_net_spanPackToOffsets(span);
  const buf = __taida_net_rawToBuffer(raw);
  const needleBuf = __taida_net_needleToBuffer(needle);
  if (!offsets || !buf || !needleBuf) { return false; }
  const start = offsets[0];
  const len = offsets[1];
  if (start + len > buf.length) { return false; }
  if (needleBuf.length === 0) { return true; }
  if (len < needleBuf.length) { return false; }
  outer: for (let i = 0; i + needleBuf.length <= len; i++) {
    for (let j = 0; j < needleBuf.length; j++) {
      if (buf[start + i + j] !== needleBuf[j]) { continue outer; }
    }
    return true;
  }
  return false;
}
function __taida_net_SpanSlice(span, raw, subStart, subEnd) {
  const offsets = __taida_net_spanPackToOffsets(span);
  const baseStart = offsets ? offsets[0] : 0;
  const baseLen = offsets ? offsets[1] : 0;
  let s = Number(subStart) | 0;
  let e = Number(subEnd) | 0;
  if (s < 0) { s = 0; }
  if (s > baseLen) { s = baseLen; }
  if (e < s) { e = s; }
  if (e > baseLen) { e = baseLen; }
  return { start: baseStart + s, len: e - s };
}

// ── C26B-016 (@c.26, Option B+): `StrOf[span, raw]()` — cold-path span → Str ──
// Materialize a span pack into an owned JS string via UTF-8 decode. Invalid
// UTF-8 or OOB span → empty string (tolerant semantics, consistent with
// Span* family). Differs from `Utf8Decode` (which returns `Lax[Str]`) — this
// returns a raw Str directly, matching the interpreter's `StrOf` mold.
function __taida_net_StrOf(span, raw) {
  const offsets = __taida_net_spanPackToOffsets(span);
  const buf = __taida_net_rawToBuffer(raw);
  if (!offsets || !buf) { return ""; }
  const start = offsets[0];
  const len = offsets[1];
  if (start + len > buf.length) { return ""; }
  if (len === 0) { return ""; }
  try {
    return buf.toString('utf8', start, start + len);
  } catch (e) {
    return "";
  }
}

// Helper: create net Result success (reuses __taida_result_create)
function __taida_net_result_ok(inner) {
  return __taida_result_create(inner, null, null);
}

// Helper: create net Result failure with kind/message
function __taida_net_result_fail(kind, message) {
  const inner = Object.freeze({ ok: false, code: -1, message: message, kind: kind });
  // E33B-003 Cat B: surface `kind` at top-level so user code reading
  // `err.kind` (after `|==` catch) matches Interpreter / Native parity.
  // Keep `fields.kind` populated for legacy `err.fields.kind` callers.
  const errVal = { __type: 'HttpError', type: 'HttpError', message: message, kind: kind, fields: { kind: kind } };
  return __taida_result_create(inner, errVal, null);
}

// Helper: create a span object @(start, len)
function __taida_net_span(start, len) {
  return Object.freeze({ start: start, len: len });
}

// Status reason phrases (mirrors Interpreter status_reason)
function __taida_net_status_reason(code) {
  const reasons = {
    100:'Continue',101:'Switching Protocols',
    200:'OK',201:'Created',202:'Accepted',204:'No Content',
    205:'Reset Content',206:'Partial Content',
    301:'Moved Permanently',302:'Found',304:'Not Modified',
    307:'Temporary Redirect',308:'Permanent Redirect',
    400:'Bad Request',401:'Unauthorized',403:'Forbidden',404:'Not Found',
    405:'Method Not Allowed',408:'Request Timeout',409:'Conflict',410:'Gone',
    413:'Content Too Large',415:'Unsupported Media Type',418:"I'm a Teapot",
    422:'Unprocessable Content',429:'Too Many Requests',
    500:'Internal Server Error',502:'Bad Gateway',503:'Service Unavailable',504:'Gateway Timeout',
  };
  return reasons[code] || '';
}

// httpParseRequestHead(bytes) -> Result[@(parsed), _]
// Parses HTTP/1.1 request head from raw bytes (Uint8Array or string).
// Returns the same shape as the Interpreter: @(complete, consumed, method, path, query, version, headers, bodyOffset, contentLength, chunked)
function __taida_net_httpParseRequestHead(input) {
  let bytes;
  if (input instanceof Uint8Array) {
    bytes = input;
  } else if (typeof input === 'string') {
    bytes = Buffer.from(input, 'utf-8');
  } else {
    return __taida_net_result_fail('ParseError', 'httpParseRequestHead: argument must be Bytes or Str');
  }

  // Find \r\n\r\n (end of head)
  let headEnd = -1;
  for (let i = 0; i <= bytes.length - 4; i++) {
    if (bytes[i] === 13 && bytes[i+1] === 10 && bytes[i+2] === 13 && bytes[i+3] === 10) {
      headEnd = i + 4;
      break;
    }
  }

  const complete = headEnd >= 0;
  const headBytes = complete ? bytes.subarray(0, headEnd) : bytes;
  const headStr = Buffer.from(headBytes).toString('latin1');

  // Split header from the rest
  const lines = headStr.split('\r\n');
  if (lines.length === 0 || lines[0].length === 0) {
    if (!complete) {
      // Incomplete: return partial with complete=false
      return __taida_net_result_ok(Object.freeze({
        complete: false, consumed: 0,
        method: __taida_net_span(0, 0), path: __taida_net_span(0, 0),
        query: __taida_net_span(0, 0), version: Object.freeze({ major: 1, minor: 1 }),
        headers: Object.freeze([]), bodyOffset: 0, contentLength: 0, chunked: false,
      }));
    }
    return __taida_net_result_fail('ParseError', 'Malformed HTTP request: empty request line');
  }

  // Parse request line: METHOD SP PATH SP HTTP/x.y
  const requestLine = lines[0];
  const sp1 = requestLine.indexOf(' ');
  if (sp1 < 0) {
    if (!complete) {
      return __taida_net_result_ok(Object.freeze({
        complete: false, consumed: 0,
        method: __taida_net_span(0, 0), path: __taida_net_span(0, 0),
        query: __taida_net_span(0, 0), version: Object.freeze({ major: 1, minor: 1 }),
        headers: Object.freeze([]), bodyOffset: 0, contentLength: 0, chunked: false,
      }));
    }
    return __taida_net_result_fail('ParseError', 'Malformed HTTP request: invalid request line');
  }
  const sp2 = requestLine.indexOf(' ', sp1 + 1);
  if (sp2 < 0) {
    if (!complete) {
      return __taida_net_result_ok(Object.freeze({
        complete: false, consumed: 0,
        method: __taida_net_span(0, sp1), path: __taida_net_span(0, 0),
        query: __taida_net_span(0, 0), version: Object.freeze({ major: 1, minor: 1 }),
        headers: Object.freeze([]), bodyOffset: 0, contentLength: 0, chunked: false,
      }));
    }
    return __taida_net_result_fail('ParseError', 'Malformed HTTP request: invalid request line');
  }

  // Method span
  const methodStart = 0;
  const methodLen = sp1;

  // Path + query (split on '?')
  const fullPath = requestLine.substring(sp1 + 1, sp2);
  const fullPathStart = sp1 + 1;
  const qIdx = fullPath.indexOf('?');
  let pathStart, pathLen, queryStart, queryLen;
  if (qIdx >= 0) {
    pathStart = fullPathStart;
    pathLen = qIdx;
    queryStart = fullPathStart + qIdx + 1;
    queryLen = fullPath.length - qIdx - 1;
  } else {
    pathStart = fullPathStart;
    pathLen = fullPath.length;
    queryStart = 0;
    queryLen = 0;
  }

  // Version (strict: must match HTTP/x.y exactly when head is complete)
  const versionStr = requestLine.substring(sp2 + 1);
  let major = 1, minor = 1;
  const vMatch = versionStr.match(/^HTTP\/(\d)\.(\d)$/);
  if (vMatch) {
    major = parseInt(vMatch[1], 10);
    minor = parseInt(vMatch[2], 10);
    // NB-32: restrict to HTTP/1.0 and HTTP/1.1 only (parity with Interpreter/httparse)
    // Reject immediately once version is fully parsed, regardless of head completeness
    if (major !== 1 || (minor !== 0 && minor !== 1)) {
      return __taida_net_result_fail('ParseError', 'Malformed HTTP request: invalid HTTP version');
    }
  } else if (complete) {
    return __taida_net_result_fail('ParseError', 'Malformed HTTP request: invalid HTTP version');
  }

  // Headers (lines[1] .. lines[n-1], stop at empty line)
  const headersList = [];
  let contentLength = 0;
  let clCount = 0;
  let hasTransferEncodingChunked = false;
  // Track byte offset of each header line for span calculation
  let lineOffset = requestLine.length + 2; // skip request line + \r\n
  for (let i = 1; i < lines.length; i++) {
    const line = lines[i];
    if (line.length === 0) break; // end of headers
    // NB-4/NB-6: enforce max 64 headers (parity with Interpreter/httparse)
    if (headersList.length >= 64) {
      return __taida_net_result_fail('ParseError', 'Malformed HTTP request: too many headers');
    }
    const colonIdx = line.indexOf(':');
    if (colonIdx < 0) {
      // Malformed header line
      if (complete) {
        return __taida_net_result_fail('ParseError', 'Malformed HTTP request: invalid header line');
      }
      break;
    }
    const nameStart = lineOffset;
    const nameLen = colonIdx;
    // Value: skip leading SP/HT after colon, and trim trailing SP/HT (NB-34: parity with Interpreter/httparse)
    let valueOff = colonIdx + 1;
    while (valueOff < line.length && (line[valueOff] === ' ' || line[valueOff] === '\t')) valueOff++;
    let valueEnd = line.length;
    while (valueEnd > valueOff && (line[valueEnd - 1] === ' ' || line[valueEnd - 1] === '\t')) valueEnd--;
    const valueStart = lineOffset + valueOff;
    const valueLen = valueEnd - valueOff;

    headersList.push(Object.freeze({
      name: __taida_net_span(nameStart, nameLen),
      value: __taida_net_span(valueStart, valueLen),
    }));

    // Check Content-Length
    const headerName = line.substring(0, colonIdx);
    if (headerName.toLowerCase() === 'content-length') {
      clCount++;
      if (clCount > 1) {
        return __taida_net_result_fail('ParseError', 'Malformed HTTP request: duplicate Content-Length header');
      }
      const rawVal = line.substring(colonIdx + 1).trim();
      // Strict: entire value must be digits (parseInt would accept "5abc" as 5)
      if (!/^\d+$/.test(rawVal)) {
        return __taida_net_result_fail('ParseError', 'Malformed HTTP request: invalid Content-Length value');
      }
      // Strip leading zeros for numeric comparison (RFC 9110: Content-Length = 1*DIGIT,
      // leading zeros are valid). Interpreter uses parse::<i64>() and Native uses manual
      // digit accumulation — both ignore leading zeros. JS must match.
      const clStripped = rawVal.replace(/^0+/, '') || '0';
      // Cap at Number.MAX_SAFE_INTEGER (2^53 - 1 = 9007199254740991) for
      // cross-backend parity. JS Number loses precision beyond this value,
      // so both backends must reject to keep contentLength identical.
      // String comparison: reject if >16 digits, or exactly 16 digits and > '9007199254740991'.
      if (clStripped.length > 16 || (clStripped.length === 16 && clStripped > '9007199254740991')) {
        return __taida_net_result_fail('ParseError', 'Malformed HTTP request: invalid Content-Length value');
      }
      const parsedCl = parseInt(rawVal, 10);
      if (isNaN(parsedCl) || parsedCl < 0) {
        return __taida_net_result_fail('ParseError', 'Malformed HTTP request: invalid Content-Length value');
      }
      contentLength = parsedCl;
    }
    // NET2-2a: Detect Transfer-Encoding: chunked (parity with Interpreter)
    if (headerName.toLowerCase() === 'transfer-encoding') {
      // Scan comma-separated tokens for "chunked" (case-insensitive)
      const tokens = line.substring(colonIdx + 1).split(',');
      for (const token of tokens) {
        if (token.trim().toLowerCase() === 'chunked') {
          hasTransferEncodingChunked = true;
        }
      }
    }
    lineOffset += line.length + 2; // +2 for \r\n
  }

  // NET2-2e: Reject Content-Length + Transfer-Encoding: chunked (RFC 7230 section 3.3.3)
  if (hasTransferEncodingChunked && clCount > 0) {
    return __taida_net_result_fail('ParseError', 'Malformed HTTP request: Content-Length and Transfer-Encoding: chunked are mutually exclusive');
  }

  const consumed = complete ? headEnd : 0;
  const parsed = Object.freeze({
    complete: complete,
    consumed: consumed,
    method: __taida_net_span(methodStart, methodLen),
    path: __taida_net_span(pathStart, pathLen),
    query: __taida_net_span(queryStart, queryLen),
    version: Object.freeze({ major: major, minor: minor }),
    headers: Object.freeze(headersList),
    bodyOffset: consumed,
    contentLength: contentLength,
    chunked: hasTransferEncodingChunked,
  });
  return __taida_net_result_ok(parsed);
}

// httpEncodeResponse(response) -> Result[@(bytes: Bytes), _]
// Encodes a response pack @(status, headers, body) into HTTP/1.1 wire bytes.
function __taida_net_httpEncodeResponse(response) {
  if (!response || typeof response !== 'object') {
    return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: argument must be a BuchiPack @(...)');
  }

  const status = response.status;
  if (typeof status !== 'number' || !Number.isInteger(status)) {
    return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: status must be Int, got ' + String(status));
  }
  if (status < 100 || status > 999) {
    return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: status must be 100-999, got ' + status);
  }

  // RFC 9110: 1xx, 204, 205, 304 MUST NOT contain a message body
  const noBody = (status >= 100 && status < 200) || status === 204 || status === 205 || status === 304;

  const headers = response.headers;
  if (!Array.isArray(headers)) {
    return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: headers must be a List, got ' + String(headers));
  }

  // Validate and collect headers
  const headerPairs = [];
  for (let i = 0; i < headers.length; i++) {
    const h = headers[i];
    if (!h || typeof h !== 'object') {
      return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: headers[' + i + '] must be @(name, value)');
    }
    const name = h.name;
    const value = h.value;
    if (typeof name !== 'string') {
      return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: headers[' + i + '].name must be Str');
    }
    if (typeof value !== 'string') {
      return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: headers[' + i + '].value must be Str');
    }
    // Length limits (parity with Interpreter/Native).
    if (Buffer.byteLength(name, 'utf-8') > 8192) {
      return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: headers[' + i + '].name exceeds 8192 bytes');
    }
    if (Buffer.byteLength(value, 'utf-8') > 65536) {
      return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: headers[' + i + '].value exceeds 65536 bytes');
    }
    // RFC 7230 token + field-value grammar (parity with the streaming validator).
    if (name.length === 0) {
      return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: headers[' + i + '].name is empty');
    }
    {
      const nameBuf = Buffer.from(name, 'utf-8');
      for (let k = 0; k < nameBuf.length; k++) {
        const b = nameBuf[k];
        if (!__taida_net_isRfc7230TokenByte(b)) {
          return __taida_net_result_fail('EncodeError',
            'httpEncodeResponse: headers[' + i + '].name contains a byte outside RFC 7230 token grammar (0x' +
            b.toString(16).toUpperCase().padStart(2, '0') + ')');
        }
      }
      if (nameBuf.includes(0x5F)) {
        return __taida_net_result_fail('EncodeError',
          "httpEncodeResponse: headers[" + i + "].name contains '_' which reverse proxies normalise inconsistently");
      }
    }
    {
      const valueBuf = Buffer.from(value, 'utf-8');
      for (let k = 0; k < valueBuf.length; k++) {
        const b = valueBuf[k];
        if (!__taida_net_isRfc7230FieldValueByte(b)) {
          return __taida_net_result_fail('EncodeError',
            'httpEncodeResponse: headers[' + i + '].value contains a byte outside RFC 7230 field-value grammar (0x' +
            b.toString(16).toUpperCase().padStart(2, '0') + ')');
        }
      }
    }
    {
      const lower = name.toLowerCase();
      if (lower === 'transfer-encoding') {
        return __taida_net_result_fail('EncodeError',
          "httpEncodeResponse: headers[" + i + "].name 'Transfer-Encoding' is runtime-managed");
      }
      if (lower === 'set-cookie') {
        return __taida_net_result_fail('EncodeError',
          "httpEncodeResponse: headers[" + i + "].name 'Set-Cookie' is reserved by the runtime");
      }
      // Content-Length: legacy behaviour — handler-supplied value flows
      // through and the encoder coalesces with its own auto-append.
    }
    headerPairs.push([name, value]);
  }

  // Body
  let bodyBytes;
  const bodyVal = response.body;
  if (bodyVal instanceof Uint8Array) {
    bodyBytes = bodyVal;
  } else if (typeof bodyVal === 'string') {
    bodyBytes = Buffer.from(bodyVal, 'utf-8');
  } else if (bodyVal === undefined || bodyVal === null) {
    return __taida_net_result_fail('EncodeError', "httpEncodeResponse: missing required field 'body'");
  } else {
    return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: body must be Bytes or Str, got ' + String(bodyVal));
  }

  if (noBody && bodyBytes.length > 0) {
    return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: status ' + status + ' must not have a body');
  }

  // Build wire bytes
  const reason = __taida_net_status_reason(status);
  let head = 'HTTP/1.1 ' + status + ' ' + reason + '\r\n';

  let hasContentLength = false;
  for (const [name, value] of headerPairs) {
    if (noBody && name.toLowerCase() === 'content-length') continue;
    head += name + ': ' + value + '\r\n';
    if (name.toLowerCase() === 'content-length') hasContentLength = true;
  }

  if (!noBody && !hasContentLength) {
    head += 'Content-Length: ' + bodyBytes.length + '\r\n';
  }
  head += '\r\n';

  const headBuf = Buffer.from(head, 'latin1');
  let result;
  if (noBody || bodyBytes.length === 0) {
    result = new Uint8Array(headBuf);
  } else {
    result = new Uint8Array(headBuf.length + bodyBytes.length);
    result.set(headBuf, 0);
    result.set(bodyBytes, headBuf.length);
  }

  return __taida_net_result_ok(Object.freeze({ bytes: result }));
}

// NB6-1: Scatter-gather send for internal one-shot response path.
// Returns { head: Buffer, body: Buffer|Uint8Array } or null on error.
// Avoids the aggregate Uint8Array concatenation of httpEncodeResponse.
function __taida_net_encodeResponseScatter(response) {
  if (!response || typeof response !== 'object') return null;
  const status = response.status;
  if (typeof status !== 'number' || !Number.isInteger(status) || status < 100 || status > 999) return null;
  const noBody = (status >= 100 && status < 200) || status === 204 || status === 205 || status === 304;
  const headers = response.headers;
  if (!Array.isArray(headers)) return null;

  let bodyBytes;
  const bodyVal = response.body;
  if (bodyVal instanceof Uint8Array) {
    bodyBytes = bodyVal;
  } else if (typeof bodyVal === 'string') {
    bodyBytes = Buffer.from(bodyVal, 'utf-8');
  } else {
    return null;
  }

  if (noBody && bodyBytes.length > 0) return null;

  const reason = __taida_net_status_reason(status);
  let head = 'HTTP/1.1 ' + status + ' ' + reason + '\r\n';
  let hasContentLength = false;
  for (let i = 0; i < headers.length; i++) {
    const h = headers[i];
    if (!h || typeof h !== 'object') return null;
    const name = h.name, value = h.value;
    if (typeof name !== 'string' || typeof value !== 'string') return null;
    if (Buffer.byteLength(name, 'utf-8') > 8192) return null;
    if (Buffer.byteLength(value, 'utf-8') > 65536) return null;
    if (name.length === 0) return null;
    // RFC 7230 token + field-value grammar (parity with public encoder
    // and streaming validator). Without this the scatter path forwarded
    // ':' / NUL / SP / HTAB / control / DEL / underscore / Set-Cookie
    // straight onto the wire.
    {
      const nameBuf = Buffer.from(name, 'utf-8');
      for (let k = 0; k < nameBuf.length; k++) {
        if (!__taida_net_isRfc7230TokenByte(nameBuf[k])) return null;
      }
      if (nameBuf.includes(0x5F)) return null; // underscore in name
      const valueBuf = Buffer.from(value, 'utf-8');
      for (let k = 0; k < valueBuf.length; k++) {
        if (!__taida_net_isRfc7230FieldValueByte(valueBuf[k])) return null;
      }
    }
    const lower = name.toLowerCase();
    if (lower === 'transfer-encoding') return null;
    if (lower === 'set-cookie') return null;
    if (lower === 'content-length') {
      if (noBody) continue;
      hasContentLength = true;
    }
    head += name + ': ' + value + '\r\n';
  }
  if (!noBody && !hasContentLength) {
    head += 'Content-Length: ' + bodyBytes.length + '\r\n';
  }
  head += '\r\n';
  return { head: Buffer.from(head, 'latin1'), body: bodyBytes };
}

// E32B-051 / E32B-052: per-line and trailer-block caps shared with
// Interpreter (helpers.rs::MAX_CHUNK_LINE_BYTES / MAX_TRAILER_COUNT /
// MAX_TRAILER_BYTES) and Native (net_h1_h2.c TAIDA_NET_MAX_*). Documented in
// docs/reference/net_api.md §5.4.
const __TAIDA_NET_MAX_CHUNK_LINE_BYTES = 1048576;
const __TAIDA_NET_MAX_TRAILER_COUNT = 64;
const __TAIDA_NET_MAX_TRAILER_BYTES = 8192;

// NET2-4b: Chunked Transfer Encoding in-place compaction (JS)
// Mirrors Interpreter's chunked_in_place_compact algorithm.
// buf is a Buffer; bodyOffset is where chunk framing starts.
// Returns { bodyLen, wireConsumed } on success, or null on malformed input.
function __taida_net_chunkedInPlaceCompact(buf, bodyOffset) {
  const dataLen = buf.length - bodyOffset;
  let readPos = 0;
  let writePos = 0;

  // Cap per-line CRLF scan at __TAIDA_NET_MAX_CHUNK_LINE_BYTES bytes (parity
  // with Native `taida_net_find_crlf(_, cap)` and Interpreter
  // `find_crlf_capped(_, cap)`). Both Rust and C scan exactly `cap` total
  // bytes (i+1 < cap), so the JS scan must use `start + cap - 1` as the
  // exclusive upper bound to match. Off-by-one between backends would let
  // attackers tune chunk-line length to exploit the most-permissive backend.
  function findCRLF(start) {
    const end = Math.min(
      dataLen - 1,
      start + __TAIDA_NET_MAX_CHUNK_LINE_BYTES - 1
    );
    for (let i = start; i < end; i++) {
      if (buf[bodyOffset + i] === 13 && buf[bodyOffset + i + 1] === 10) return i;
    }
    return -1;
  }

  for (;;) {
    // Find CRLF after chunk-size
    const crlfPos = findCRLF(readPos);
    if (crlfPos < 0) return null; // malformed: missing CRLF

    // Parse chunk-size (hex), ignoring chunk-ext after semicolon
    let hexEnd = crlfPos;
    for (let i = readPos; i < crlfPos; i++) {
      if (buf[bodyOffset + i] === 0x3B) { hexEnd = i; break; } // ';'
    }
    // E32B-053: per RFC 7230 §4.1 chunk-size MUST NOT contain OWS.
    // No SP/HT trim — strict hex validation below rejects any whitespace.
    let hexStart = readPos;

    if (hexStart >= hexEnd) return null; // empty chunk-size

    const hexStr = buf.toString('latin1', bodyOffset + hexStart, bodyOffset + hexEnd);
    if (!/^[0-9a-fA-F]+$/.test(hexStr)) return null; // strict hex validation
    // NB2-4: Reject oversized chunk-size (parity with body_complete).
    // Leading-zero policy: 15-digit cap is enforced on the literal hex
    // length so leading zeros count toward the cap.
    if (hexStr.length > 15) return null; // malformed: oversized chunk-size
    const chunkSize = parseInt(hexStr, 16);
    if (isNaN(chunkSize) || chunkSize < 0 || !Number.isSafeInteger(chunkSize)) return null; // invalid hex

    // Advance past "chunk-size\r\n"
    readPos = crlfPos + 2;

    // Terminator chunk (size == 0)
    if (chunkSize === 0) {
      // Skip optional trailer headers until final CRLF, bounded by line
      // count + total bytes (E32B-052) and per-line length (E32B-051).
      let trailerCount = 0;
      let trailerBytes = 0;
      for (;;) {
        if (readPos + 2 > dataLen) return null; // malformed: missing final CRLF
        if (buf[bodyOffset + readPos] === 13 && buf[bodyOffset + readPos + 1] === 10) {
          readPos += 2;
          return { bodyLen: writePos, wireConsumed: readPos };
        }
        if (trailerCount >= __TAIDA_NET_MAX_TRAILER_COUNT) return null; // too many trailers
        // Skip trailer line
        const trlf = findCRLF(readPos);
        if (trlf < 0) return null; // malformed: incomplete trailer
        const lineLen = trlf - readPos;
        trailerBytes += lineLen;
        if (trailerBytes > __TAIDA_NET_MAX_TRAILER_BYTES) return null; // trailer block too large
        trailerCount++;
        readPos = trlf + 2;
      }
    }

    // Validate: enough data for chunk-data + CRLF
    if (readPos + chunkSize + 2 > dataLen) return null; // truncated

    // In-place compaction: copy chunk data to write position.
    // Buffer.copy handles overlapping regions safely (memmove equivalent).
    if (writePos !== readPos) {
      buf.copy(buf, bodyOffset + writePos, bodyOffset + readPos, bodyOffset + readPos + chunkSize);
    }
    writePos += chunkSize;
    readPos += chunkSize;

    // Validate trailing CRLF after chunk data
    if (buf[bodyOffset + readPos] !== 13 || buf[bodyOffset + readPos + 1] !== 10) return null;
    readPos += 2;
  }
}

// NET2-4b: Check if a complete chunked body is available in the buffer (read-only scan).
// Returns wireConsumed (bytes from bodyOffset to end of last chunk + trailers) or -1 if incomplete, -2 if malformed.
function __taida_net_chunkedBodyComplete(buf, bodyOffset) {
  const dataLen = buf.length - bodyOffset;
  let readPos = 0;

  for (;;) {
    if (readPos >= dataLen) return -1; // need more data

    // Find CRLF after chunk-size, capped at MAX_CHUNK_LINE_BYTES so a chunk
    // -ext flood is treated as malformed rather than "incomplete" (E32B-051).
    // Off-by-one parity with Rust/C: we scan `cap` bytes total, i.e. up to
    // absolute index `readPos + cap - 1` inclusive, hence `i < readPos + cap - 1`.
    let crlfPos = -1;
    const scanWindow = Math.min(
      dataLen - 1,
      readPos + __TAIDA_NET_MAX_CHUNK_LINE_BYTES - 1
    );
    for (let i = readPos; i < scanWindow; i++) {
      if (buf[bodyOffset + i] === 13 && buf[bodyOffset + i + 1] === 10) { crlfPos = i; break; }
    }
    if (crlfPos < 0) {
      const remaining = dataLen - readPos;
      if (remaining >= __TAIDA_NET_MAX_CHUNK_LINE_BYTES) return -2; // malformed
      return -1; // need more data
    }

    // Parse chunk-size hex
    let hexEnd = crlfPos;
    for (let i = readPos; i < crlfPos; i++) {
      if (buf[bodyOffset + i] === 0x3B) { hexEnd = i; break; }
    }
    // E32B-053: no OWS trim; whitespace inside chunk-size is rejected by
    // the strict hex regex below, matching Interpreter / Native parity.
    let hexStart = readPos;
    if (hexStart >= hexEnd) return -2; // malformed: empty chunk-size

    const hexStr = buf.toString('latin1', bodyOffset + hexStart, bodyOffset + hexEnd);
    if (!/^[0-9a-fA-F]+$/.test(hexStr)) return -2; // strict hex validation (rejects OWS)
    // NB2-4: Reject oversized chunk-size that would exceed safe integer range.
    // Leading-zero policy: 15-digit cap is enforced on literal hex length;
    // leading zeros count toward the cap so `00...01` (16 digits) is rejected.
    if (hexStr.length > 15) return -2; // malformed: oversized chunk-size
    const chunkSize = parseInt(hexStr, 16);
    if (isNaN(chunkSize) || chunkSize < 0 || !Number.isSafeInteger(chunkSize)) return -2; // malformed

    readPos = crlfPos + 2;

    if (chunkSize === 0) {
      // Skip trailers, bounded by line count + total bytes (E32B-052) and
      // per-line length (E32B-051). Hitting a cap is treated as malformed.
      let trailerCount = 0;
      let trailerBytes = 0;
      for (;;) {
        if (readPos + 2 > dataLen) return -1;
        if (buf[bodyOffset + readPos] === 13 && buf[bodyOffset + readPos + 1] === 10) {
          return readPos + 2; // complete
        }
        if (trailerCount >= __TAIDA_NET_MAX_TRAILER_COUNT) return -2;
        let trlf = -1;
        const tWindow = Math.min(
          dataLen - 1,
          readPos + __TAIDA_NET_MAX_CHUNK_LINE_BYTES - 1
        );
        for (let i = readPos; i < tWindow; i++) {
          if (buf[bodyOffset + i] === 13 && buf[bodyOffset + i + 1] === 10) { trlf = i; break; }
        }
        if (trlf < 0) {
          const tRemaining = dataLen - readPos;
          if (tRemaining >= __TAIDA_NET_MAX_CHUNK_LINE_BYTES) return -2;
          return -1;
        }
        const lineLen = trlf - readPos;
        trailerBytes += lineLen;
        if (trailerBytes > __TAIDA_NET_MAX_TRAILER_BYTES) return -2;
        trailerCount++;
        readPos = trlf + 2;
      }
    }

    if (readPos + chunkSize + 2 > dataLen) return -1; // need more data
    readPos += chunkSize;
    if (buf[bodyOffset + readPos] !== 13 || buf[bodyOffset + readPos + 1] !== 10) return -2; // malformed
    readPos += 2;
  }
}

// NET2-4a: Determine keep-alive from parsed headers and HTTP version.
// raw: Buffer, headers: array of {name: span, value: span}, httpMinor: 0 or 1
function __taida_net_determineKeepAlive(raw, headers, httpMinor) {
  let hasClose = false;
  let hasKeepAlive = false;
  for (const hdr of headers) {
    const ns = hdr.name.start;
    const nl = hdr.name.len;
    if (ns + nl > raw.length) continue;
    const nameStr = raw.toString('latin1', ns, ns + nl).toLowerCase();
    if (nameStr === 'connection') {
      const vs = hdr.value.start;
      const vl = hdr.value.len;
      if (vs + vl > raw.length) continue;
      const valStr = raw.toString('latin1', vs, vs + vl);
      const tokens = valStr.split(',');
      for (const token of tokens) {
        const t = token.trim().toLowerCase();
        if (t === 'close') hasClose = true;
        else if (t === 'keep-alive') hasKeepAlive = true;
      }
    }
  }
  // RFC 7230 section 6.1: close always wins
  if (hasClose) return false;
  // HTTP/1.1: keep-alive by default; HTTP/1.0: close by default
  return httpMinor === 1 ? true : hasKeepAlive;
}

// httpServe(port, handler, maxRequests?, timeoutMs?, maxConnections?, tls?) -> Async[Result[@(ok, requests), _]]
// NB4-7: Monotonic request token counter for identity verification.
let __taida_net_requestTokenCounter = 0;
function __taida_net_nextRequestToken() {
  return ++__taida_net_requestTokenCounter;
}

// NET2-4a/4b/4c/4d: TCP server with keep-alive, chunked TE, concurrent connections, maxConnections.
// v5: tls parameter added (6th arg). @() or undefined = plaintext, @(cert, key) = HTTPS (Phase 2 stub).
// Node.js event loop provides natural concurrency (multiple sockets active simultaneously).
// bind to 127.0.0.1 (never 0.0.0.0). maxRequests=0 means unlimited.
async function __taida_net_httpServe(port, handler, maxRequests, timeoutMs, maxConnections, tls) {
  if (typeof port !== 'number' || !Number.isInteger(port) || port < 0 || port > 65535) {
    return new __TaidaAsync(
      __taida_net_result_fail('BindError', 'httpServe: port must be 0-65535, got ' + String(port)),
      null, 'fulfilled');
  }
  if (typeof handler !== 'function') {
    return new __TaidaAsync(
      __taida_net_result_fail('TypeError', 'httpServe: handler must be a Function'),
      null, 'fulfilled');
  }
  const maxReq = (typeof maxRequests === 'number' && Number.isInteger(maxRequests)) ? maxRequests : 0;
  // NB-9: timeoutMs <= 0 falls back to 5000ms (v1 default).
  // socket.setTimeout(0) means "disable timeout" in Node.js = wait forever; 0 must not reach the socket.
  const timeout = (typeof timeoutMs === 'number' && Number.isInteger(timeoutMs) && timeoutMs > 0) ? timeoutMs : 5000;
  // NET2-4d: maxConnections (optional, default 128). <= 0 falls back to 128.
  const maxConn = (typeof maxConnections === 'number' && Number.isInteger(maxConnections) && maxConnections > 0) ? maxConnections : 128;

  // v5: TLS configuration.
  // tls is a BuchiPack (object) or undefined/null.
  // @() = empty object = plaintext (v4 compat).
  // @(cert: "path", key: "path") = HTTPS.
  // v6 NET6-1b: @(cert: ..., key: ..., protocol: "h2") = HTTP/2 (rejected on JS).
  let __useTls = false;
  let __tlsCert = null;
  let __tlsKey = null;
  let __requestedProtocol = null;
  if (tls !== undefined && tls !== null && typeof tls === 'object') {
    // v6 NET6-1b: Extract protocol field if present.
    // NB6-10: Separate "field exists" from "field is Str".
    // If protocol field exists but is not Str, reject immediately.
    if ('protocol' in tls) {
      if (typeof tls.protocol === 'string') {
        __requestedProtocol = tls.protocol;
      } else if (typeof tls.protocol === 'number' && Number.isInteger(tls.protocol)) {
        // Sync with `crate::net_surface::http_protocol_ordinal_to_wire`.
        if (tls.protocol === 0) {
          __requestedProtocol = 'h1.1';
        } else if (tls.protocol === 1) {
          __requestedProtocol = 'h2';
        } else if (tls.protocol === 2) {
          __requestedProtocol = 'h3';
        } else {
          return new __TaidaAsync(
            __taida_net_result_fail('ProtocolError',
              'httpServe: unknown HttpProtocol ordinal ' + tls.protocol +
              '. Expected 0 (H1), 1 (H2), or 2 (H3).'),
            null, 'fulfilled');
        }
      } else {
        return new __TaidaAsync(
          __taida_net_result_fail('ProtocolError',
            'httpServe: protocol must be HttpProtocol or Str, got ' + typeof tls.protocol),
          null, 'fulfilled');
      }
    }
    // NB7-6: Check h2/h3 unsupported BEFORE cert/key file load so that
    // backend contract errors (H2Unsupported, H3Unsupported) are returned
    // instead of TlsError when cert/key files are invalid or missing.
    // JS is a permanent h1-only compatibility backend (v6/v7 design lock).
    if (__requestedProtocol === 'h2') {
      return new __TaidaAsync(
        __taida_net_result_fail('H2Unsupported',
          'httpServe: HTTP/2 (protocol: "h2") is not supported on the JS backend. ' +
          'Use the interpreter or native backend for HTTP/2 support.'),
        null, 'fulfilled');
    }
    if (__requestedProtocol === 'h3') {
      return new __TaidaAsync(
        __taida_net_result_fail('H3Unsupported',
          'httpServe: HTTP/3 (protocol: "h3") is not supported on the JS backend. ' +
          'Use the native or interpreter backend for HTTP/3 support.'),
        null, 'fulfilled');
    }
    const hasCert = 'cert' in tls;
    const hasKey = 'key' in tls;
    if (hasCert || hasKey) {
      // Validate that both cert and key are present and are Str.
      if (hasCert && !hasKey) {
        return new __TaidaAsync(
          __taida_net_result_fail('TlsError', 'httpServe: tls.key must be a Str (PEM file path)'),
          null, 'fulfilled');
      }
      if (!hasCert && hasKey) {
        return new __TaidaAsync(
          __taida_net_result_fail('TlsError', 'httpServe: tls.cert must be a Str (PEM file path)'),
          null, 'fulfilled');
      }
      if (typeof tls.cert !== 'string') {
        return new __TaidaAsync(
          __taida_net_result_fail('TlsError', 'httpServe: tls.cert must be a Str (PEM file path)'),
          null, 'fulfilled');
      }
      if (typeof tls.key !== 'string') {
        return new __TaidaAsync(
          __taida_net_result_fail('TlsError', 'httpServe: tls.key must be a Str (PEM file path)'),
          null, 'fulfilled');
      }
      // v5 Phase 3: Load cert/key files at startup (NET5-0c: startup failure = Result failure).
      if (!__taida_fs) {
        return new __TaidaAsync(
          __taida_net_result_fail('TlsError', 'httpServe: fs module not available for TLS cert/key loading'),
          null, 'fulfilled');
      }
      if (!__os_tls) {
        return new __TaidaAsync(
          __taida_net_result_fail('TlsError', 'httpServe: tls module not available'),
          null, 'fulfilled');
      }
      try {
        __tlsCert = __taida_fs.readFileSync(tls.cert);
      } catch (e) {
        return new __TaidaAsync(
          __taida_net_result_fail('TlsError', 'httpServe: failed to read cert file: ' + (e.message || e)),
          null, 'fulfilled');
      }
      try {
        __tlsKey = __taida_fs.readFileSync(tls.key);
      } catch (e) {
        return new __TaidaAsync(
          __taida_net_result_fail('TlsError', 'httpServe: failed to read key file: ' + (e.message || e)),
          null, 'fulfilled');
      }
      __useTls = true;
    } else if (__requestedProtocol !== null) {
      // v6 NET6-1b: @(protocol: "h2") without cert/key — still validate protocol.
      // Fall through to protocol validation below.
    }
    // else: empty object @() → plaintext, fall through
  } else if (tls !== undefined && tls !== null) {
    // NB5-16: non-object tls (e.g. 42, "str", true) must NOT silently fall back to plaintext.
    // Match Interpreter parity: RuntimeError for invalid tls type.
    throw new __NativeError('httpServe: tls must be a BuchiPack @(cert: Str, key: Str) or @(), got ' + typeof tls);
  }

  // v6 NET6-1b / v7 NET7-1c: Protocol validation (remaining checks).
  // h2/h3 unsupported checks were hoisted above cert/key loading (NB7-6).
  // This block handles h1.1 passthrough and unknown protocol rejection.
  if (__requestedProtocol !== null) {
    if (__requestedProtocol === 'h1.1' || __requestedProtocol === 'http/1.1') {
      // Explicit HTTP/1.1 — same as default, no action needed.
    } else {
      // Unknown protocol (h2/h3 already handled above cert/key load).
      return new __TaidaAsync(
        __taida_net_result_fail('ProtocolError',
          'httpServe: unknown protocol "' + __requestedProtocol + '". Supported values: "h1.1", "h2", "h3"'),
        null, 'fulfilled');
    }
  }

  const net = __os_net;
  if (!net) {
    return new __TaidaAsync(
      __taida_net_result_fail('BindError', 'httpServe: net module not available'),
      null, 'fulfilled');
  }

  return new Promise((resolveOuter) => {
    let requestCount = 0;
    let serverClosed = false;
    // NET2-4c/4d: Track active connections for maxConnections enforcement
    let activeConnections = 0;
    const MAX_REQUEST_BUF = 1048576;

    // v5: Create TLS or plaintext server based on tls parameter.
    let server;
    if (__useTls) {
      // TLS server using node:tls. The 'secureConnection' event provides
      // a tls.TLSSocket (decrypted stream) that has the same API as net.Socket.
      try {
        server = __os_tls.createServer({
          cert: __tlsCert,
          key: __tlsKey,
          // Disable client certificate verification (server-only TLS).
          requestCert: false,
          // Allow self-signed certificates (validation is client's responsibility).
          rejectUnauthorized: false,
        });
      } catch (e) {
        resolveOuter(new __TaidaAsync(
          __taida_net_result_fail('TlsError', 'httpServe: failed to create TLS server: ' + (e.message || e)),
          null, 'fulfilled'));
        return;
      }
    } else {
      server = net.createServer({ allowHalfOpen: false });
    }
    // NET2-4d: Use Node.js built-in maxConnections to limit simultaneous connections.
    // When at capacity, Node.js queues incoming connections in the kernel backlog.
    server.maxConnections = maxConn;

    function finish(ok) {
      if (serverClosed) return;
      serverClosed = true;
      server.close(() => {});
      const inner = Object.freeze({ ok: ok, requests: requestCount });
      resolveOuter(new __TaidaAsync(__taida_net_result_ok(inner), null, 'fulfilled'));
    }

    function connClosed() {
      activeConnections--;
    }

    // NET2-4a/4b/4c: Process a single connection with keep-alive loop.
    // Each connection runs independently (Node.js event loop concurrency).
    function processConnection(socket) {
      activeConnections++;
      // NB5-23: Pre-allocated growable buffer with amortized doubling.
      // Previous approach used Buffer.concat([buf, chunk]) per data event,
      // copying all existing bytes each time = O(n^2) for n chunks.
      // Now we maintain a backing buffer (_bufBacking) with spare capacity,
      // and expose buf as a subarray view of the valid data region.
      // Appending a chunk copies only the chunk (not existing data) when
      // capacity suffices, and doubles the backing buffer otherwise —
      // amortized O(1) per byte.
      // NB5-23: Pre-allocated growable buffer with amortized doubling.
      // Previous approach used Buffer.concat([buf, chunk]) per data event,
      // copying all existing bytes each time = O(n^2) for n chunks.
      // Now we maintain a backing buffer with spare capacity.
      // bufAppend() copies only the chunk (not existing data) when capacity
      // suffices, and doubles the backing buffer otherwise — amortized O(1)
      // per byte. bufConsume(n) advances the valid region without reallocation.
      let _bb = Buffer.alloc(8192); // backing buffer
      let _bo = 0; // offset of valid data start within _bb
      let _bl = 0; // length of valid data
      function bufAppend(chunk) {
        if (_bo + _bl + chunk.length <= _bb.length) {
          chunk.copy(_bb, _bo + _bl);
          _bl += chunk.length;
        } else if (_bl + chunk.length <= _bb.length) {
          // Compact: move valid data to start, then append.
          _bb.copy(_bb, 0, _bo, _bo + _bl);
          _bo = 0;
          chunk.copy(_bb, _bl);
          _bl += chunk.length;
        } else {
          // Grow: double until sufficient, copy valid + chunk.
          let newCap = _bb.length * 2;
          while (newCap < _bl + chunk.length) newCap *= 2;
          const nb = Buffer.alloc(newCap);
          _bb.copy(nb, 0, _bo, _bo + _bl);
          chunk.copy(nb, _bl);
          _bb = nb;
          _bo = 0;
          _bl += chunk.length;
        }
        buf = _bb.subarray(_bo, _bo + _bl);
      }
      function bufConsume(n) {
        _bo += n;
        _bl -= n;
        if (_bl <= 0) { _bo = 0; _bl = 0; }
        buf = _bb.subarray(_bo, _bo + _bl);
      }
      function bufReset() {
        _bo = 0; _bl = 0;
        buf = _bb.subarray(0, 0);
      }
      let buf = _bb.subarray(0, 0);
      let connClosed_ = false;
      let connRequests = 0;

      function closeConn() {
        if (connClosed_) return;
        connClosed_ = true;
        socket.removeAllListeners();
        // NB5-11: For TLS sockets, socket.write() is asynchronous — the TLS
        // layer encrypts data in the event loop. Calling socket.destroy()
        // immediately would discard pending TLS writes (response body, chunked
        // terminator, close_notify). Use socket.end() which flushes all queued
        // writes to the TLS layer before closing. The 'close' event will fire
        // after the socket is fully closed, which is harmless.
        if (socket.__tls && !socket.destroyed) {
          socket.end();
          // Ensure the socket is destroyed after a short timeout in case
          // end() stalls (e.g., unresponsive client).
          setTimeout(() => {
            if (!socket.destroyed) socket.destroy();
          }, 1000);
        } else {
          socket.destroy();
        }
        connClosed();
      }

      function send400AndClose() {
        if (connClosed_) return;
        connClosed_ = true;
        socket.removeAllListeners();
        if (!socket.destroyed && socket.writable) {
          socket.write('HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n', () => {
            socket.destroy();
          });
        } else {
          socket.destroy();
        }
        requestCount++;
        if (maxReq > 0 && requestCount >= maxReq) { connClosed(); finish(true); return; }
        connClosed();
      }

      function send413AndClose() {
        if (connClosed_) return;
        connClosed_ = true;
        socket.removeAllListeners();
        if (!socket.destroyed && socket.writable) {
          socket.write('HTTP/1.1 413 Content Too Large\r\nContent-Length: 0\r\nConnection: close\r\n\r\n', () => {
            socket.destroy();
          });
        } else {
          socket.destroy();
        }
        requestCount++;
        if (maxReq > 0 && requestCount >= maxReq) { connClosed(); finish(true); return; }
        connClosed();
      }

      function send500AndClose(msg) {
        if (connClosed_) return;
        connClosed_ = true;
        socket.removeAllListeners();
        const errBody = 'Internal Server Error: ' + String(msg);
        if (!socket.destroyed && socket.writable) {
          socket.write('HTTP/1.1 500 Internal Server Error\r\nContent-Length: ' + Buffer.byteLength(errBody) + '\r\nConnection: close\r\n\r\n' + errBody, () => {
            socket.destroy();
          });
        } else {
          socket.destroy();
        }
        requestCount++;
        if (maxReq > 0 && requestCount >= maxReq) { connClosed(); finish(true); return; }
        connClosed();
      }

      // Try to process a complete request from the current buffer.
      // Returns true if a request was dispatched (async handler may still be running).
      // Returns false if we need more data.
      function tryProcessRequest() {
        if (connClosed_ || serverClosed) return false;

        // Check if head is complete
        let headEnd = -1;
        for (let i = 0; i <= buf.length - 4; i++) {
          if (buf[i] === 13 && buf[i+1] === 10 && buf[i+2] === 13 && buf[i+3] === 10) {
            headEnd = i + 4;
            break;
          }
        }
        if (headEnd < 0) return false; // need more data

        // NB2-18: Pass buf directly (Buffer IS-A Uint8Array, no copy needed)
        const parseResult = __taida_net_httpParseRequestHead(buf);
        const parsed = parseResult && parseResult.__value;
        if (!parsed || (parseResult.throw !== null && parseResult.throw !== undefined)) {
          send400AndClose(); return true;
        }
        if (!parsed.complete) return false; // need more data

        const isChunked = parsed.chunked || false;
        const contentLength = isChunked ? 0 : (parsed.contentLength || 0);

        // NET4-3a: Detect handler arity to decide body-deferred vs eager path.
        const handlerArity = handler.length;

        if (handlerArity >= 2) {
          // ── v4 2-arg handler: body-deferred path (NB4-16 fix) ──

          // NB5-11 fix: For TLS sockets, pre-buffer the entire body before
          // dispatching the handler. Node.js TLS sockets deliver decrypted data
          // via event loop callbacks; synchronous busy-poll (sock.read() in a
          // tight loop) cannot receive data that arrives after handler dispatch
          // because the event loop is blocked. Pre-buffering ensures all body
          // bytes are available as leftover before the synchronous handler runs.
          // For plaintext sockets, the original fd-based synchronous I/O works
          // correctly and body-deferred streaming is preserved.
          if (socket.__tls && (contentLength > 0 || isChunked)) {
            // NB6-2: TLS + body present: pre-buffer entire body before dispatch.
            // Design contract: TLS streaming body is non-zero-copy / non-streaming
            // due to Node.js TLS sockets delivering via event loop callbacks.
            // This is a fundamental limitation of the sync handler model.
            // HTTP/2 will require async runtime boundary (out of v6 scope for JS).
            if (!isChunked) {
              // Content-Length path: wait until buf has head + full body.
              const bodyNeeded = parsed.consumed + contentLength;
              if (buf.length < bodyNeeded) return false; // need more body data

              // NB6-2: Use buf.slice() for owned copies (avoids intermediate
              // Buffer.subarray view + Buffer.from double-copy overhead).
              const remoteAddr = socket.remoteAddress || '127.0.0.1';
              const cleanHost = remoteAddr.startsWith('::ffff:') ? remoteAddr.substring(7) : remoteAddr;
              const keepAlive = __taida_net_determineKeepAlive(buf, parsed.headers, parsed.version.minor);
              const rawSnapshot = buf.slice(0, parsed.consumed);
              const leftover = buf.slice(parsed.consumed, bodyNeeded);
              bufConsume(bodyNeeded);

              const request = {
                raw: new Uint8Array(rawSnapshot.buffer, rawSnapshot.byteOffset, rawSnapshot.byteLength),
                method: parsed.method,
                path: parsed.path,
                query: parsed.query,
                version: parsed.version,
                headers: parsed.headers,
                body: __taida_net_span(0, 0),
                bodyOffset: parsed.consumed,
                contentLength: contentLength,
                remoteHost: cleanHost,
                remotePort: socket.remotePort || 0,
                keepAlive: keepAlive,
                chunked: false,
                __body_stream: '__v4_body_stream',
                __body_token: __taida_net_nextRequestToken(),
                _socket: socket,
                __tls_prebuffered: true,
              };

              dispatchHandlerBodyDeferred(request, keepAlive, leftover, false, contentLength);
              return true;
            } else {
              // Chunked path: wait until terminal chunk (0\r\n...\r\n) is in buf.
              const completeness = __taida_net_chunkedBodyComplete(buf, parsed.consumed);
              if (completeness === -1) return false; // need more data
              if (completeness === -2) { send400AndClose(); return true; } // malformed

              // Full chunked body is in buf. Compact and extract leftover.
              const compact = __taida_net_chunkedInPlaceCompact(buf, parsed.consumed);
              if (!compact) { send400AndClose(); return true; }

              const totalWire = parsed.consumed + compact.wireConsumed;
              const remoteAddr = socket.remoteAddress || '127.0.0.1';
              const cleanHost = remoteAddr.startsWith('::ffff:') ? remoteAddr.substring(7) : remoteAddr;
              const keepAlive = __taida_net_determineKeepAlive(buf, parsed.headers, parsed.version.minor);
              // NB6-2: Use buf.slice() for owned copies (avoids Buffer.from + subarray overhead).
              const rawSnapshot = buf.slice(0, parsed.consumed);
              const leftover = buf.slice(parsed.consumed, parsed.consumed + compact.bodyLen);
              bufConsume(totalWire);

              const request = {
                raw: new Uint8Array(rawSnapshot.buffer, rawSnapshot.byteOffset, rawSnapshot.byteLength),
                method: parsed.method,
                path: parsed.path,
                query: parsed.query,
                version: parsed.version,
                headers: parsed.headers,
                body: __taida_net_span(0, 0),
                bodyOffset: parsed.consumed,
                contentLength: compact.bodyLen,
                remoteHost: cleanHost,
                remotePort: socket.remotePort || 0,
                keepAlive: keepAlive,
                chunked: true,
                __body_stream: '__v4_body_stream',
                __body_token: __taida_net_nextRequestToken(),
                _socket: socket,
                __tls_prebuffered: true,
              };

              dispatchHandlerBodyDeferred(request, keepAlive, leftover, true, compact.bodyLen);
              return true;
            }
          }

          // Plaintext or TLS with no body: dispatch at HEAD arrival time.
          // Body bytes are read incrementally via readBodyChunk/readBodyAll.
          // Any body bytes that arrived with the head buffer are passed
          // as leftover; remaining bytes are read via fs.readSync when
          // readBodyChunk/readBodyAll is called.

          const remoteAddr = socket.remoteAddress || '127.0.0.1';
          const cleanHost = remoteAddr.startsWith('::ffff:') ? remoteAddr.substring(7) : remoteAddr;
          const keepAlive = __taida_net_determineKeepAlive(buf, parsed.headers, parsed.version.minor);

          // Capture only the head as raw (body is NOT in raw for 2-arg handlers).
          const rawSnapshot = buf.slice(0, parsed.consumed);

          // Capture any body bytes that arrived with the head parse buffer.
          const leftover = buf.length > parsed.consumed
            ? buf.slice(parsed.consumed)
            : Buffer.alloc(0);

          const request = {
            raw: new Uint8Array(rawSnapshot.buffer, rawSnapshot.byteOffset, rawSnapshot.byteLength),
            method: parsed.method,
            path: parsed.path,
            query: parsed.query,
            version: parsed.version,
            headers: parsed.headers,
            body: __taida_net_span(0, 0),
            bodyOffset: parsed.consumed,
            contentLength: contentLength,
            remoteHost: cleanHost,
            remotePort: socket.remotePort || 0,
            keepAlive: keepAlive,
            chunked: isChunked,
            __body_stream: '__v4_body_stream',
            __body_token: __taida_net_nextRequestToken(),
            _socket: socket,
          };

          // Clear buf — all buffered bytes are either in rawSnapshot or leftover.
          bufReset();
          dispatchHandlerBodyDeferred(request, keepAlive, leftover, isChunked, contentLength);
          return true;
        }

        // ── v2 1-arg handler: eager body read (unchanged) ──

        if (isChunked) {
          // NET2-4b: Chunked Transfer Encoding path
          const completeness = __taida_net_chunkedBodyComplete(buf, parsed.consumed);
          if (completeness === -1) return false; // need more data
          if (completeness === -2) { send400AndClose(); return true; } // malformed

          // Perform in-place compaction
          const compact = __taida_net_chunkedInPlaceCompact(buf, parsed.consumed);
          if (!compact) { send400AndClose(); return true; } // malformed

          const totalWire = parsed.consumed + compact.wireConsumed;
          // Detach request-scoped raw (owned copy): head + compacted body
          const rawLen = parsed.consumed + compact.bodyLen;
          const raw = buf.subarray(0, rawLen);

          const remoteAddr = socket.remoteAddress || '127.0.0.1';
          const cleanHost = remoteAddr.startsWith('::ffff:') ? remoteAddr.substring(7) : remoteAddr;
          // NB2-18: Determine keepAlive from buf directly (no extra copy)
          const keepAlive = __taida_net_determineKeepAlive(buf, parsed.headers, parsed.version.minor);

          // Snapshot raw for request pack (owned copy, decoupled from scratch buffer)
          const rawSnapshot = Buffer.from(raw);
          const request = Object.freeze({
            raw: new Uint8Array(rawSnapshot.buffer, rawSnapshot.byteOffset, rawSnapshot.byteLength),
            method: parsed.method,
            path: parsed.path,
            query: parsed.query,
            version: parsed.version,
            headers: parsed.headers,
            body: __taida_net_span(parsed.consumed, compact.bodyLen),
            bodyOffset: parsed.consumed,
            contentLength: compact.bodyLen,
            remoteHost: cleanHost,
            remotePort: socket.remotePort || 0,
            keepAlive: keepAlive,
            chunked: true,
          });

          // NB5-23: Advance buffer using amortized consume.
          bufConsume(totalWire);

          dispatchHandler(request, keepAlive);
          return true;
        } else {
          // Content-Length path
          // NB-3: Early reject if head + body exceeds buffer limit (413 Content Too Large)
          if (parsed.consumed + contentLength > MAX_REQUEST_BUF) { send413AndClose(); return true; }

          const bodyNeeded = parsed.consumed + contentLength;
          if (buf.length < bodyNeeded) return false; // need more body data

          const remoteAddr = socket.remoteAddress || '127.0.0.1';
          const cleanHost = remoteAddr.startsWith('::ffff:') ? remoteAddr.substring(7) : remoteAddr;
          // NB2-18: Determine keepAlive from buf directly (no extra copy)
          const keepAlive = __taida_net_determineKeepAlive(buf, parsed.headers, parsed.version.minor);

          // Snapshot raw for request pack (owned copy, decoupled from scratch buffer)
          const rawSnapshot = buf.slice(0, bodyNeeded);
          const request = Object.freeze({
            raw: new Uint8Array(rawSnapshot.buffer, rawSnapshot.byteOffset, rawSnapshot.byteLength),
            method: parsed.method,
            path: parsed.path,
            query: parsed.query,
            version: parsed.version,
            headers: parsed.headers,
            body: __taida_net_span(parsed.consumed, contentLength),
            bodyOffset: parsed.consumed,
            contentLength: contentLength,
            remoteHost: cleanHost,
            remotePort: socket.remotePort || 0,
            keepAlive: keepAlive,
            chunked: false,
          });

          // NB5-23: Advance buffer using amortized consume.
          bufConsume(bodyNeeded);

          dispatchHandler(request, keepAlive);
          return true;
        }
      }

      // Dispatch handler call and manage keep-alive continuation.
      function dispatchHandler(request, keepAlive) {
        // Pause data events while handling (sequential within connection)
        socket.pause();
        socket.removeAllListeners('data');
        socket.removeAllListeners('timeout');
        socket.removeAllListeners('end');

        // NET3-4a: Detect handler arity (1-arg vs 2-arg).
        // handler.length gives the number of declared parameters.
        const handlerArity = handler.length;

        if (handlerArity >= 2) {
          // ── v3 2-arg handler path ──
          // Create a writer object with mutable state for streaming.
          const writer = {
            __writer_id: '__v3_streaming_writer',
            _state: 0,           // 0=Idle, 1=HeadPrepared, 2=Streaming, 3=Ended
            _pendingStatus: 200,
            _pendingHeaders: [],  // Array of @(name, value)
            _sseMode: false,
            _socket: socket,
            _needsDrain: false,   // backpressure flag: set when sock.write returns false
          };
          // Listen for drain events to clear backpressure flag.
          // Attached once per request (removed in afterResponseWritten
          // to prevent keep-alive accumulation).
          function onDrain() {
            writer._needsDrain = false;
          }
          socket.on('drain', onDrain);
          writer._onDrain = onDrain; // stash for removal

          let responseVal;
          try {
            responseVal = handler(request, writer);
            if (responseVal && typeof responseVal.then === 'function') {
              responseVal.then((val) => {
                afterHandlerStreaming(val, keepAlive, writer);
              }).catch((err) => {
                afterHandlerStreamingError(err, keepAlive, writer);
              });
              return;
            }
          } catch (err) {
            afterHandlerStreamingError(err, keepAlive, writer);
            return;
          }
          afterHandlerStreaming(responseVal, keepAlive, writer);
        } else {
          // ── v2 1-arg handler path (unchanged) ──
          let responseVal;
          try {
            responseVal = handler(request);
            if (responseVal && typeof responseVal.then === 'function') {
              responseVal.then((val) => {
                afterHandler(val, keepAlive);
              }).catch((err) => {
                send500AndClose(err && err.message || err);
              });
              return;
            }
          } catch (err) {
            send500AndClose(err && err.message || err);
            return;
          }
          afterHandler(responseVal, keepAlive);
        }
      }

      // NET4-3a: Dispatch handler with body-deferred mode for 2-arg handlers.
      // Body is NOT eagerly read — readBodyChunk/readBodyAll will read from socket.
      function dispatchHandlerBodyDeferred(request, keepAlive, leftover, isChunked, contentLength) {
        // Pause data events while handling (sequential within connection)
        socket.pause();
        socket.removeAllListeners('data');
        socket.removeAllListeners('timeout');
        socket.removeAllListeners('end');

        // NB5-11: For TLS pre-buffered requests, all body bytes are already
        // in leftover. The body is decoded (chunked framing removed) and
        // presented as a Content-Length body so readBodyChunk/readBodyAll
        // consume from leftover only — no socket I/O during the synchronous
        // handler. bytesConsumed starts at 0; the normal CL read path will
        // drain leftover and set fullyRead when bytesConsumed >= contentLength.
        const tlsPreBuffered = request.__tls_prebuffered === true;

        // Create writer with body state for v4 body-deferred mode.
        const writer = {
          __writer_id: '__v3_streaming_writer',
          _state: 0,           // 0=Idle, 1=HeadPrepared, 2=Streaming, 3=Ended, 4=WebSocket
          _pendingStatus: 200,
          _pendingHeaders: [],
          _sseMode: false,
          _socket: socket,
          _needsDrain: false,
          // v4: body streaming state
          _bodyState: {
            isChunked: tlsPreBuffered ? false : isChunked,
            contentLength: contentLength,
            bytesConsumed: 0,
            fullyRead: !isChunked && contentLength === 0,
            anyReadStarted: false,
            leftover: leftover,    // leftover body bytes from head parse buffer
            leftoverPos: 0,
            // Chunked decoder state: 'waitSize' | 'readData' | 'waitTrailer' | 'done'
            chunkedState: tlsPreBuffered ? 'done' : 'waitSize',
            chunkedRemaining: 0,
            requestToken: request.__body_token,
          },
          // v4: WebSocket state
          _wsClosed: false,
          _wsCloseCode: 0, // v5: 0 = no close frame received yet
        };

        function onDrain() {
          writer._needsDrain = false;
        }
        socket.on('drain', onDrain);
        writer._onDrain = onDrain;

        // Store writer on socket so readBodyChunk/readBodyAll/ws* can find it.
        socket.__v4_writer = writer;

        let responseVal;
        try {
          responseVal = handler(request, writer);
          if (responseVal && typeof responseVal.then === 'function') {
            responseVal.then((val) => {
              afterHandlerStreamingV4(val, keepAlive, writer);
            }).catch((err) => {
              afterHandlerStreamingErrorV4(err, keepAlive, writer);
            });
            return;
          }
        } catch (err) {
          afterHandlerStreamingErrorV4(err, keepAlive, writer);
          return;
        }
        afterHandlerStreamingV4(responseVal, keepAlive, writer);
      }

      // NET4-3a: Error handler for v4 body-deferred 2-arg handler.
      function afterHandlerStreamingErrorV4(err, keepAlive, writer) {
        const msg = (err && err.message) || String(err);
        socket.__v4_writer = null;
        __taida_net_activeWsWriter = null;

        // v4: WebSocket state — send close frame on error.
        if (writer._state === 4) {
          if (!writer._wsClosed && !socket.destroyed && socket.writable) {
            // Send close frame with 1011 (internal error).
            __taida_net_writeWsFrame(socket, 0x8, Buffer.from([0x03, 0xF3]));
          }
          requestCount++;
          closeConn();
          if (maxReq > 0 && requestCount >= maxReq) { finish(true); }
          return;
        }

        if (writer._state === 2) {
          if (!socket.destroyed && socket.writable) {
            socket.write('0\r\n\r\n', () => { closeConn(); });
          } else {
            closeConn();
          }
          writer._state = 3;
          requestCount++;
          return;
        }
        if (writer._state === 3) {
          requestCount++;
          closeConn();
          return;
        }
        writer._state = 3;
        send500AndClose(msg);
      }

      // NET4-3a: afterHandler for v4 body-deferred 2-arg handler.
      function afterHandlerStreamingV4(responseVal, keepAlive, writer) {
        socket.__v4_writer = null;
        __taida_net_activeWsWriter = null;
        if (connClosed_ || serverClosed) return;
        if (socket.destroyed || !socket.writable) { closeConn(); return; }

        // v4: WebSocket auto-close on handler return.
        if (writer._state === 4) {
          if (!writer._wsClosed && !socket.destroyed && socket.writable) {
            // Auto close with 1000 (normal closure).
            __taida_net_writeWsFrame(socket, 0x8, Buffer.from([0x03, 0xE8]));
          }
          requestCount++;
          connRequests++;
          // WebSocket connections never return to keep-alive.
          closeConn();
          if (maxReq > 0 && requestCount >= maxReq) { finish(true); }
          return;
        }

        if (writer._state === 0) {
          // ── One-shot fallback: writer never touched ──
          const isResponsePack = responseVal && typeof responseVal === 'object'
            && ('status' in responseVal || 'body' in responseVal);
          const effectiveResponse = isResponsePack ? responseVal
            : Object.freeze({ status: 200, headers: Object.freeze([]), body: '' });

          // NB6-1: Scatter-gather send — head and body as separate buffers.
          // Use cork/uncork to batch both writes into a single TCP segment.
          const scatter = __taida_net_encodeResponseScatter(effectiveResponse);
          if (scatter) {
            if (scatter.body.length > 0) {
              socket.cork();
              socket.write(scatter.head);
              socket.write(scatter.body, () => {
                afterResponseWrittenV4(keepAlive, writer);
              });
              socket.uncork();
            } else {
              socket.write(scatter.head, () => {
                afterResponseWrittenV4(keepAlive, writer);
              });
            }
          } else {
            socket.write(Buffer.from('HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n'), () => {
              afterResponseWrittenV4(false, writer);
            });
          }
        } else {
          // Streaming was started. Return value is ignored.
          // Auto-end if not already ended.
          if (writer._state !== 3) {
            if (writer._state === 1) {
              const headBytes = __taida_net_buildStreamingHead(writer._pendingStatus, writer._pendingHeaders);
              socket.write(headBytes);
            }
            if (!__taida_net_isBodylessStatus(writer._pendingStatus)) {
              writer._state = 3;
              socket.write('0\r\n\r\n', () => {
                afterResponseWrittenV4(keepAlive, writer);
              });
              return;
            }
            writer._state = 3;
          }
          afterResponseWrittenV4(keepAlive, writer);
        }
      }

      // v4 keep-alive continuation with unread body check.
      function afterResponseWrittenV4(keepAlive, writer) {
        requestCount++;
        connRequests++;

        if (maxReq > 0 && requestCount >= maxReq) {
          closeConn();
          finish(true);
          return;
        }

        // NET4-1g: If body was not fully read, close (no keep-alive).
        const bs = writer._bodyState;
        const bodyDone = bs.fullyRead || (!bs.isChunked && bs.contentLength === 0);
        if (!bodyDone || !keepAlive) {
          closeConn();
          return;
        }

        // Body was fully consumed; safe to continue keep-alive.
        if (connClosed_ || serverClosed || socket.destroyed) { closeConn(); return; }

        // NB5-24: Recover trailing bytes from body state leftover.
        // When a pipelined client sends the next request in the same TCP segment
        // as the current body, those bytes are in leftover beyond the consumed body.
        // Prepend them to the connection buffer so the next request can be parsed.
        if (bs.leftover && bs.leftoverPos < bs.leftover.length) {
          const trailing = bs.leftover.subarray(bs.leftoverPos);
          if (trailing.length > 0) {
            bufAppend(trailing);
          }
        }

        if (buf.length > 0 && tryProcessRequest()) return;

        socket.removeAllListeners('drain');
        socket.removeAllListeners('timeout');
        socket.removeAllListeners('end');
        socket.removeAllListeners('error');

        socket.setTimeout(timeout);
        socket.on('timeout', () => {
          if (buf.length > 0) {
            send400AndClose();
          } else {
            closeConn();
          }
        });
        socket.on('end', () => {
          if (buf.length > 0) {
            send400AndClose();
          } else {
            closeConn();
          }
        });
        socket.on('error', () => { closeConn(); });
        socket.on('data', onData);
        socket.resume();
      }

      // NET3-4a: Handle error in 2-arg handler
      function afterHandlerStreamingError(err, keepAlive, writer) {
        const msg = (err && err.message) || String(err);
        if (writer._state === 2) {
          // Head already committed — send chunk terminator and close
          if (!socket.destroyed && socket.writable) {
            socket.write('0\r\n\r\n', () => { closeConn(); });
          } else {
            closeConn();
          }
          writer._state = 3;
          requestCount++;
          return;
        }
        if (writer._state === 3) {
          // Already ended — just close
          requestCount++;
          closeConn();
          return;
        }
        // Head not yet committed (Idle/HeadPrepared) — safe to send 500
        writer._state = 3;
        send500AndClose(msg);
      }

      // NET3-4a: afterHandler for 2-arg handler (streaming path)
      function afterHandlerStreaming(responseVal, keepAlive, writer) {
        if (connClosed_ || serverClosed) return;
        if (socket.destroyed || !socket.writable) { closeConn(); return; }

        if (writer._state === 0) {
          // ── One-shot fallback: writer never touched ──
          // Use responseVal as v2-style response pack, or default 200 + empty body.
          const isResponsePack = responseVal && typeof responseVal === 'object'
            && ('status' in responseVal || 'body' in responseVal);
          const effectiveResponse = isResponsePack ? responseVal
            : Object.freeze({ status: 200, headers: Object.freeze([]), body: '' });

          // NB6-1: Scatter-gather send — head and body as separate buffers.
          const scatter2 = __taida_net_encodeResponseScatter(effectiveResponse);
          if (scatter2) {
            if (scatter2.body.length > 0) {
              socket.cork();
              socket.write(scatter2.head);
              socket.write(scatter2.body, () => {
                afterResponseWritten(keepAlive);
              });
              socket.uncork();
            } else {
              socket.write(scatter2.head, () => {
                afterResponseWritten(keepAlive);
              });
            }
          } else {
            socket.write(Buffer.from('HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n'), () => {
              afterResponseWritten(false);
            });
          }
        } else {
          // Streaming was started. Return value is ignored.
          // Auto-end if not already ended.
          if (writer._state !== 3) {
            if (writer._state === 1) {
              // HeadPrepared but never wrote chunks — commit head first
              const headBytes = __taida_net_buildStreamingHead(writer._pendingStatus, writer._pendingHeaders);
              socket.write(headBytes);
            }
            // Send chunked terminator (only for non-bodyless status).
            // Use callback on the last write to ensure data is flushed
            // before afterResponseWritten potentially closes the connection.
            if (!__taida_net_isBodylessStatus(writer._pendingStatus)) {
              writer._state = 3;
              socket.write('0\r\n\r\n', () => {
                afterResponseWritten(keepAlive);
              });
              return;
            }
            writer._state = 3;
          }
          // Streaming response done — continue keep-alive loop
          afterResponseWritten(keepAlive);
        }
      }

      function afterHandler(responseVal, keepAlive) {
        if (connClosed_ || serverClosed) return;
        // NB2-12: Guard against writing to a destroyed/ended socket
        if (socket.destroyed || !socket.writable) { closeConn(); return; }

        // NB6-1: Scatter-gather send — head and body as separate buffers.
        const scatter3 = __taida_net_encodeResponseScatter(responseVal);
        if (scatter3) {
          if (scatter3.body.length > 0) {
            socket.cork();
            socket.write(scatter3.head);
            socket.write(scatter3.body, () => {
              afterResponseWritten(keepAlive);
            });
            socket.uncork();
          } else {
            socket.write(scatter3.head, () => {
              afterResponseWritten(keepAlive);
            });
          }
        } else {
          socket.write(Buffer.from('HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n'), () => {
            afterResponseWritten(false);
          });
        }
      }

      // Shared keep-alive continuation after any response (one-shot or streaming)
      function afterResponseWritten(keepAlive) {
          requestCount++;
          connRequests++;

          // Check maxRequests
          if (maxReq > 0 && requestCount >= maxReq) {
            closeConn();
            finish(true);
            return;
          }

          // Keep-alive decision
          if (!keepAlive) {
            closeConn();
            return;
          }

          // NET2-4a: Continue keep-alive loop — re-attach listeners for next request
          if (connClosed_ || serverClosed || socket.destroyed) { closeConn(); return; }

          // Check if there is already a complete request in the leftover buffer
          // (pipelined or buffered data)
          if (buf.length > 0 && tryProcessRequest()) return;

          // NB2-8: Remove all existing listeners before re-attaching to prevent
          // listener accumulation on keep-alive connections (avoids MaxListenersExceededWarning).
          socket.removeAllListeners('drain');
          socket.removeAllListeners('timeout');
          socket.removeAllListeners('end');
          socket.removeAllListeners('error');

          // Re-attach data listener for next request on this connection
          socket.setTimeout(timeout);
          socket.on('timeout', () => {
            if (buf.length > 0) {
              // Partial data timeout: bad request
              send400AndClose();
            } else {
              // True idle on keep-alive: clean close
              closeConn();
            }
          });
          // NB2-3: Partial data on keep-alive follow-up should be 400,
          // not silent close — parity with Interpreter/Native.
          socket.on('end', () => {
            if (buf.length > 0) {
              send400AndClose();
            } else {
              closeConn();
            }
          });
          socket.on('error', () => { closeConn(); });
          socket.on('data', onData);
          socket.resume();
      }

      function onData(chunk) {
        if (connClosed_ || serverClosed) { closeConn(); return; }
        // NB5-23: amortized O(1) append instead of O(n) Buffer.concat.
        bufAppend(chunk);
        if (buf.length > MAX_REQUEST_BUF) { send400AndClose(); return; }
        tryProcessRequest();
      }

      // NB2-2: Initial event setup for the first request.
      // If no data has arrived (buf.length === 0), idle timeout/EOF is a clean close
      // (no 400, no request budget consumed) — parity with Interpreter/Native.
      socket.setTimeout(timeout);
      socket.on('timeout', () => {
        if (buf.length > 0) {
          send400AndClose();
        } else {
          closeConn();
        }
      });
      socket.on('end', () => {
        if (!connClosed_) {
          if (buf.length > 0) send400AndClose();
          else closeConn();
        }
      });
      socket.on('error', () => { closeConn(); });
      socket.on('data', onData);
    }

    server.on('error', (err) => {
      if (serverClosed) return;
      serverClosed = true;
      server.close(() => {});
      resolveOuter(new __TaidaAsync(
        __taida_net_result_fail('BindError', 'httpServe: failed to bind to 127.0.0.1:' + port + ': ' + err.message),
        null, 'fulfilled'));
    });

    if (__useTls) {
      // v5 TLS: 'secureConnection' fires after successful TLS handshake.
      // The socket is a tls.TLSSocket (decrypted stream, same API as net.Socket).
      // NET5-0c: TLS handshake failure = 'tlsClientError' event → connection close, handler not called.
      server.on('tlsClientError', (err, tlsSocket) => {
        // Handshake failure: close connection, don't call handler.
        if (tlsSocket && !tlsSocket.destroyed) tlsSocket.destroy();
      });
      server.on('secureConnection', (socket) => {
        if (serverClosed) { socket.destroy(); return; }
        // Mark TLS socket so I/O helpers know to avoid raw fd access.
        socket.__tls = true;
        processConnection(socket);
      });
    } else {
      // NET2-4c: Each connection is processed independently (event-driven concurrency)
      server.on('connection', (socket) => {
        if (serverClosed) { socket.destroy(); return; }
        processConnection(socket);
      });
    }

    // C27B-014: opt-in port announcement for soak proxy / runbook.
    // Default OFF. When TAIDA_NET_ANNOUNCE_PORT=1, emit one stdout
    // line with the actually-bound port (resolved via server.address()
    // so port=0 callers learn the OS-assigned value). 3-backend parity
    // with interpreter / native (h1 + h2) on env var name + surface.
    server.on('listening', () => {
      try {
        if (typeof process !== 'undefined' && process.env && process.env.TAIDA_NET_ANNOUNCE_PORT === '1') {
          const addr = server.address();
          if (addr && typeof addr === 'object' && typeof addr.port === 'number') {
            const host = addr.address || '127.0.0.1';
            console.log('listening on ' + host + ':' + addr.port);
          }
        }
      } catch (_) { /* swallow — announcement is best-effort */ }
    });

    server.listen(port, '127.0.0.1');
  });
}

// NB2-13: __taida_net_sendResponse removed (dead code since v2 inlined response encoding in afterHandler)

// ── v3 streaming helpers ──────────────────────────────────────────

// Check if a status code forbids a message body (1xx, 204, 205, 304).
function __taida_net_isBodylessStatus(status) {
  return (status >= 100 && status <= 199) || status === 204 || status === 205 || status === 304;
}

// Map HTTP status code to reason phrase (parity with interpreter).
function __taida_net_statusReasonPhrase(status) {
  switch (status) {
    case 100: return 'Continue';
    case 101: return 'Switching Protocols';
    case 200: return 'OK';
    case 201: return 'Created';
    case 202: return 'Accepted';
    case 204: return 'No Content';
    case 205: return 'Reset Content';
    case 301: return 'Moved Permanently';
    case 302: return 'Found';
    case 304: return 'Not Modified';
    case 400: return 'Bad Request';
    case 401: return 'Unauthorized';
    case 403: return 'Forbidden';
    case 404: return 'Not Found';
    case 405: return 'Method Not Allowed';
    case 408: return 'Request Timeout';
    case 413: return 'Content Too Large';
    case 500: return 'Internal Server Error';
    case 502: return 'Bad Gateway';
    case 503: return 'Service Unavailable';
    default: return 'Unknown';
  }
}

// Build HTTP response head bytes for streaming response.
// Appends Transfer-Encoding: chunked for non-bodyless status codes.
function __taida_net_buildStreamingHead(status, headers) {
  const reason = __taida_net_statusReasonPhrase(status);
  let head = 'HTTP/1.1 ' + status + ' ' + reason + '\r\n';
  for (let i = 0; i < headers.length; i++) {
    const h = headers[i];
    head += (h.name || '') + ': ' + (h.value || '') + '\r\n';
  }
  // Auto-append Transfer-Encoding: chunked for status codes that allow body
  if (!__taida_net_isBodylessStatus(status)) {
    head += 'Transfer-Encoding: chunked\r\n';
  }
  head += '\r\n';
  return head;
}

// RFC 7230 §3.2.6 token grammar — used by both streaming + eager validators.
function __taida_net_isRfc7230TokenByte(b) {
  return (
    (b >= 0x30 && b <= 0x39) || // 0-9
    (b >= 0x41 && b <= 0x5A) || // A-Z
    (b >= 0x61 && b <= 0x7A) || // a-z
    b === 0x21 || b === 0x23 || b === 0x24 || b === 0x25 || b === 0x26 ||
    b === 0x27 || b === 0x2A || b === 0x2B || b === 0x2D || b === 0x2E ||
    b === 0x5E || b === 0x5F || b === 0x60 || b === 0x7C || b === 0x7E
  );
}

// RFC 7230 §3.2 field-value byte = HTAB / SP / VCHAR / obs-text.
function __taida_net_isRfc7230FieldValueByte(b) {
  return b === 0x09 || (b >= 0x20 && b <= 0x7E) || (b >= 0x80 && b <= 0xFF);
}

// Validate user headers for the streaming path.
function __taida_net_validateStreamingHeaders(headers) {
  for (let i = 0; i < headers.length; i++) {
    const h = headers[i];
    if (!h || typeof h !== 'object' || Array.isArray(h)) {
      throw new __NativeError('startResponse: headers[' + i + '] must be @(name, value)');
    }
    const name = h.name;
    const value = h.value;
    if (typeof name !== 'string') {
      throw new __NativeError('startResponse: headers[' + i + '].name must be Str');
    }
    if (typeof value !== 'string') {
      throw new __NativeError('startResponse: headers[' + i + '].value must be Str');
    }
    if (Buffer.byteLength(name, 'utf-8') > 8192) {
      throw new __NativeError('startResponse: headers[' + i + '].name exceeds 8192 bytes');
    }
    if (Buffer.byteLength(value, 'utf-8') > 65536) {
      throw new __NativeError('startResponse: headers[' + i + '].value exceeds 65536 bytes');
    }
    if (name.length === 0) {
      throw new __NativeError('startResponse: headers[' + i + '].name is empty');
    }
    const nameBuf = Buffer.from(name, 'utf-8');
    for (let k = 0; k < nameBuf.length; k++) {
      const b = nameBuf[k];
      if (!__taida_net_isRfc7230TokenByte(b)) {
        throw new __NativeError(
          'startResponse: headers[' + i + '].name contains a byte outside RFC 7230 token grammar (0x' +
          b.toString(16).toUpperCase().padStart(2, '0') + ')');
      }
    }
    if (nameBuf.includes(0x5F /* '_' */)) {
      throw new __NativeError(
        "startResponse: headers[" + i + "].name contains '_' which reverse proxies normalise inconsistently");
    }
    const valueBuf = Buffer.from(value, 'utf-8');
    for (let k = 0; k < valueBuf.length; k++) {
      const b = valueBuf[k];
      if (!__taida_net_isRfc7230FieldValueByte(b)) {
        throw new __NativeError(
          'startResponse: headers[' + i + '].value contains a byte outside RFC 7230 field-value grammar (0x' +
          b.toString(16).toUpperCase().padStart(2, '0') + ')');
      }
    }

    const lower = name.toLowerCase();
    if (lower === 'content-length') {
      throw new __NativeError(
        "startResponse: 'Content-Length' is not allowed in streaming response headers. " +
        'The runtime manages Content-Length/Transfer-Encoding for streaming responses.');
    }
    if (lower === 'transfer-encoding') {
      throw new __NativeError(
        "startResponse: 'Transfer-Encoding' is not allowed in streaming response headers. " +
        'The runtime manages Transfer-Encoding for streaming responses.');
    }
    if (lower === 'set-cookie') {
      throw new __NativeError(
        "startResponse: 'Set-Cookie' is reserved by the runtime; " +
        'handler-supplied Set-Cookie headers would let attacker-influenced names ' +
        '(forwarded via untrusted input) inject cookies.');
    }
  }
}

// Validate writer token: must have __writer_id === '__v3_streaming_writer'
function __taida_net_validateWriter(writer, apiName) {
  if (!writer || typeof writer !== 'object' || writer.__writer_id !== '__v3_streaming_writer') {
    throw new __NativeError(apiName + ': first argument must be the writer provided by httpServe');
  }
}

// ── v3 streaming API ─────────────────────────────────────────────

// NET3-4b: startResponse(writer, status, headers)
// Updates pending status/headers. Does NOT commit to wire.
function __taida_net_startResponse(writer, status, headers) {
  __taida_net_validateWriter(writer, 'startResponse');

  // State check
  if (writer._state === 4) {
    throw new __NativeError('startResponse: cannot use HTTP streaming API after WebSocket upgrade.');
  }
  if (writer._state === 1) {
    throw new __NativeError('startResponse: already called. Cannot call startResponse twice.');
  }
  if (writer._state === 2) {
    throw new __NativeError(
      'startResponse: head already committed (chunks are being written). ' +
      'Cannot change status/headers after writeChunk.');
  }
  if (writer._state === 3) {
    throw new __NativeError('startResponse: response already ended.');
  }

  // Default status = 200
  const s = (typeof status === 'number' && Number.isInteger(status)) ? status : 200;
  if (s < 100 || s > 599) {
    throw new __NativeError('startResponse: status must be 100-599, got ' + s);
  }

  // Default headers = []
  let h;
  if (arguments.length < 3 || typeof headers === 'undefined') {
    h = [];
  } else if (Array.isArray(headers)) {
    h = headers;
  } else {
    throw new __NativeError('startResponse: headers must be a List, got ' + String(headers));
  }

  // Validate streaming response headers
  __taida_net_validateStreamingHeaders(h);

  writer._pendingStatus = s;
  writer._pendingHeaders = h;
  writer._state = 1; // HeadPrepared

  return undefined; // Unit
}

// NET3-4b/4c/4d: writeChunk(writer, data)
// Sends one chunk of body data using chunked transfer encoding.
// Uses socket.cork()/uncork() to coalesce prefix+payload+suffix into one TCP segment.
// No Buffer.concat — each piece is written separately within a cork.
function __taida_net_writeChunk(writer, data) {
  __taida_net_validateWriter(writer, 'writeChunk');

  // State check
  if (writer._state === 4) {
    throw new __NativeError('writeChunk: cannot use HTTP streaming API after WebSocket upgrade.');
  }
  if (writer._state === 3) {
    throw new __NativeError('writeChunk: response already ended.');
  }

  // Extract payload
  let payload;
  if (data instanceof Uint8Array) {
    payload = data; // Bytes fast path (zero-copy: Buffer IS-A Uint8Array)
  } else if (typeof data === 'string') {
    payload = data; // Str — socket.write accepts strings directly (UTF-8 by default)
  } else {
    throw new __NativeError('writeChunk: data must be Bytes or Str, got ' + __taida_format(data));
  }

  // Empty chunk is no-op (avoid colliding with terminator)
  const payloadLen = (typeof payload === 'string') ? Buffer.byteLength(payload) : payload.length;
  if (payloadLen === 0) return undefined;

  // Bodyless status check
  if (__taida_net_isBodylessStatus(writer._pendingStatus)) {
    throw new __NativeError('writeChunk: status ' + writer._pendingStatus + ' does not allow a message body');
  }

  const sock = writer._socket;

  // Commit head if not yet committed
  if (writer._state === 0 || writer._state === 1) {
    const headBytes = __taida_net_buildStreamingHead(writer._pendingStatus, writer._pendingHeaders);
    sock.write(headBytes);
    writer._state = 2; // Streaming
  }

  // NET3-4c/4d: Send chunk using cork/uncork (no Buffer.concat).
  // Wire format: <hex-size>\r\n<payload>\r\n
  // Send chunk using cork/uncork (no Buffer.concat).
  // Track drain state: if sock.write returns false the kernel buffer
  // is full and the 'drain' event will fire to clear _needsDrain.
  // writeChunk always returns undefined (Unit) per NET_DESIGN contract.
  // Backpressure is handled by Node.js internal buffering; the drain
  // listener resets the flag for observability but no Promise is exposed.
  const hexPrefix = payloadLen.toString(16) + '\r\n';
  sock.cork();
  sock.write(hexPrefix);
  sock.write(payload);
  const ok = sock.write('\r\n');
  sock.uncork();
  if (!ok) {
    writer._needsDrain = true;
  }

  return undefined; // Unit
}

// NET3-4b: endResponse(writer)
// Terminates the chunked response by sending 0\r\n\r\n.
// Idempotent: second call is no-op.
function __taida_net_endResponse(writer) {
  __taida_net_validateWriter(writer, 'endResponse');

  // v4: WebSocket state check.
  if (writer._state === 4) {
    throw new __NativeError('endResponse: cannot use HTTP streaming API after WebSocket upgrade.');
  }

  // Idempotent
  if (writer._state === 3) return undefined;

  const sock = writer._socket;

  // Commit head if not yet committed
  if (writer._state === 0 || writer._state === 1) {
    const headBytes = __taida_net_buildStreamingHead(writer._pendingStatus, writer._pendingHeaders);
    sock.write(headBytes);
  }

  // Send chunked terminator (only for non-bodyless status)
  if (!__taida_net_isBodylessStatus(writer._pendingStatus)) {
    sock.write('0\r\n\r\n');
  }
  writer._state = 3; // Ended

  return undefined; // Unit
}

// NET3-4e: sseEvent(writer, event, data)
// SSE convenience API. Sends one Server-Sent Event in wire format.
// Auto-sets Content-Type and Cache-Control headers if not already set.
// Multiline data is split into multiple data: lines.
function __taida_net_sseEvent(writer, event, data) {
  __taida_net_validateWriter(writer, 'sseEvent');

  // v4: WebSocket state check.
  if (writer._state === 4) {
    throw new __NativeError('sseEvent: cannot use HTTP streaming API after WebSocket upgrade.');
  }

  if (typeof event !== 'string') {
    throw new __NativeError('sseEvent: event must be Str, got ' + __taida_format(event));
  }
  if (typeof data !== 'string') {
    throw new __NativeError('sseEvent: data must be Str, got ' + __taida_format(data));
  }

  // State check
  if (writer._state === 3) {
    throw new __NativeError('sseEvent: response already ended.');
  }

  // Bodyless status check
  if (__taida_net_isBodylessStatus(writer._pendingStatus)) {
    throw new __NativeError('sseEvent: status ' + writer._pendingStatus + ' does not allow a message body');
  }

  // NET3-3b/3c: Auto-set SSE headers if not in sse_mode
  if (!writer._sseMode) {
    if (writer._state === 2) {
      // Head already committed — check if SSE headers were set by user
      const hasSSEContentType = writer._pendingHeaders.some(function(h) {
        return (h.name || '').toLowerCase() === 'content-type'
          && (h.value || '').toLowerCase().indexOf('text/event-stream') >= 0;
      });
      const hasCacheNoCache = writer._pendingHeaders.some(function(h) {
        return (h.name || '').toLowerCase() === 'cache-control'
          && (h.value || '').toLowerCase().indexOf('no-cache') >= 0;
      });
      if (!hasSSEContentType || !hasCacheNoCache) {
        throw new __NativeError(
          'sseEvent: head already committed without SSE headers. ' +
          'Call sseEvent before writeChunk, or use startResponse ' +
          'with explicit Content-Type: text/event-stream and ' +
          'Cache-Control: no-cache headers before writeChunk.');
      }
      writer._sseMode = true;
    } else {
      // Head not yet committed — safe to add auto-headers
      const hasContentType = writer._pendingHeaders.some(function(h) {
        return (h.name || '').toLowerCase() === 'content-type';
      });
      if (!hasContentType) {
        writer._pendingHeaders.push(Object.freeze({
          name: 'Content-Type',
          value: 'text/event-stream; charset=utf-8'
        }));
      }
      const hasCacheControl = writer._pendingHeaders.some(function(h) {
        return (h.name || '').toLowerCase() === 'cache-control';
      });
      if (!hasCacheControl) {
        writer._pendingHeaders.push(Object.freeze({
          name: 'Cache-Control',
          value: 'no-cache'
        }));
      }
      writer._sseMode = true;
    }
  }

  const sock = writer._socket;

  // Commit head if not yet committed
  if (writer._state === 0 || writer._state === 1) {
    const headBytes = __taida_net_buildStreamingHead(writer._pendingStatus, writer._pendingHeaders);
    sock.write(headBytes);
    writer._state = 2; // Streaming
  }

  // Build SSE event as separate pieces (no aggregate string).
  // Wire format:
  //   event: <event>\n      (omit if empty)
  //   data: <line1>\n
  //   data: <line2>\n
  //   \n                    (event terminator)
  const dataLines = data.split('\n');

  // Compute total payload byte length from parts (without building one big string).
  let payloadLen = 0;
  if (event.length > 0) {
    payloadLen += 7 + Buffer.byteLength(event) + 1; // 'event: ' + event + '\n'
  }
  for (let i = 0; i < dataLines.length; i++) {
    payloadLen += 6 + Buffer.byteLength(dataLines[i]) + 1; // 'data: ' + line + '\n'
  }
  payloadLen += 1; // terminator '\n'

  // Send as one chunked frame using cork (pieces written separately).
  const hexPrefix = payloadLen.toString(16) + '\r\n';
  sock.cork();
  sock.write(hexPrefix);
  if (event.length > 0) {
    sock.write('event: ' + event + '\n');
  }
  for (let i = 0; i < dataLines.length; i++) {
    sock.write('data: ' + dataLines[i] + '\n');
  }
  sock.write('\n');
  const ok = sock.write('\r\n');
  sock.uncork();
  if (!ok) {
    writer._needsDrain = true;
  }

  return undefined; // Unit
}

// readBody(req) -> Bytes
// Extract body bytes from a request pack using raw buffer + body span.
// v4: In a 2-arg handler (body-deferred), acts as readBodyAll alias.
// Returns empty Uint8Array if body.len == 0.
function __taida_net_readBody(req) {
  if (!req || typeof req !== 'object') {
    throw new __NativeError('readBody: argument must be a request pack @(...), got ' + __taida_format(req));
  }

  // v4: If the request has __body_stream sentinel (2-arg handler),
  // delegate to readBodyAll to stream from socket.
  if (req.__body_stream === '__v4_body_stream') {
    return __taida_net_readBodyAll(req);
  }

  const raw = req.raw;
  if (!(raw instanceof Uint8Array)) {
    throw new __NativeError("readBody: request pack missing 'raw: Bytes' field");
  }
  const body = req.body;
  if (!body || typeof body.start !== 'number' || typeof body.len !== 'number' || body.len === 0) {
    return new Uint8Array(0);
  }
  const start = Math.max(0, body.start);
  const end = Math.min(raw.length, start + body.len);
  if (start >= end) return new Uint8Array(0);
  return raw.slice(start, end);
}

// ── v4 Request Body Streaming Helpers (synchronous) ─────────────
// NB4-16 fix: Body is dispatched at HEAD arrival. readBodyChunk/readBodyAll
// first drain leftover bytes, then read incrementally from the socket
// via fs.readSync. This eliminates full-body buffering for 2-arg handlers.

// Read one byte from leftover buffer or socket (synchronous).
// Returns -1 on EOF.
function __taida_net_readOneByte(writer) {
  const bs = writer._bodyState;
  // First drain leftover.
  if (bs.leftoverPos < bs.leftover.length) {
    return bs.leftover[bs.leftoverPos++];
  }
  // Read from socket.
  const sock = writer._socket;
  if (!sock) return -1;

  // v5 TLS: use socket.read() from decrypted buffer.
  if (sock.__tls) {
    sock.resume();
    const deadline = Date.now() + 10000;
    while (true) {
      if (Date.now() > deadline) { sock.pause(); return -1; }
      const chunk = sock.read(1);
      if (chunk && chunk.length > 0) { sock.pause(); return chunk[0]; }
      if (sock.destroyed || !sock.readable) { sock.pause(); return -1; }
      const spinEnd = Date.now() + 1;
      while (Date.now() < spinEnd) {}
    }
  }

  // Plaintext: use fd-based sync read.
  const fd = sock._handle ? sock._handle.fd : -1;
  if (fd < 0 || !__taida_fs) return -1;
  const oneBuf = Buffer.alloc(1);
  const deadline = Date.now() + 10000;
  while (true) {
    if (Date.now() > deadline) return -1;
    try {
      const n = __taida_fs.readSync(fd, oneBuf, 0, 1);
      if (n === 0) return -1; // EOF
      return oneBuf[0];
    } catch (e) {
      if (e.code === 'EAGAIN' || e.code === 'EWOULDBLOCK') {
        const spinEnd = Date.now() + 1;
        while (Date.now() < spinEnd) {}
        continue;
      }
      return -1;
    }
  }
}

// Read up to `count` bytes from leftover buffer, then socket.
// Returns a Buffer (synchronous).
function __taida_net_readBodyBytes(writer, count) {
  const bs = writer._bodyState;
  const parts = [];
  let totalRead = 0;

  // First drain leftover.
  const leftoverAvail = bs.leftover.length - bs.leftoverPos;
  if (leftoverAvail > 0) {
    const fromLeftover = Math.min(count, leftoverAvail);
    parts.push(Buffer.from(bs.leftover.subarray(bs.leftoverPos, bs.leftoverPos + fromLeftover)));
    bs.leftoverPos += fromLeftover;
    totalRead += fromLeftover;
  }

  // Then read from socket if needed.
  if (totalRead < count) {
    const sock = writer._socket;
    if (sock && sock.__tls) {
      // v5 TLS: read from decrypted stream buffer, not raw fd.
      const remaining = count - totalRead;
      sock.resume();
      const deadline = Date.now() + 10000;
      let tlsRead = 0;
      const tlsParts = [];
      while (tlsRead < remaining) {
        if (Date.now() > deadline) break;
        const chunk = sock.read(remaining - tlsRead);
        if (chunk) {
          tlsParts.push(chunk);
          tlsRead += chunk.length;
        } else {
          if (sock.destroyed || !sock.readable) break;
          const spinEnd = Date.now() + 1;
          while (Date.now() < spinEnd) {}
        }
      }
      sock.pause();
      if (tlsRead > 0) {
        const tlsBuf = tlsParts.length === 1 ? tlsParts[0] : Buffer.concat(tlsParts);
        parts.push(tlsBuf);
        totalRead += tlsRead;
      }
    } else {
      const fd = sock && sock._handle ? sock._handle.fd : -1;
      if (fd >= 0 && __taida_fs) {
        const remaining = count - totalRead;
        const fdBuf = Buffer.alloc(remaining);
        let fdPos = 0;
        const deadline = Date.now() + 10000;
        while (fdPos < remaining) {
          if (Date.now() > deadline) break;
          try {
            const n = __taida_fs.readSync(fd, fdBuf, fdPos, remaining - fdPos);
            if (n === 0) break; // EOF
            fdPos += n;
          } catch (e) {
            if (e.code === 'EAGAIN' || e.code === 'EWOULDBLOCK') {
              if (fdPos > 0) break; // return what we have
              const spinEnd = Date.now() + 1;
              while (Date.now() < spinEnd) {}
              continue;
            }
            break;
          }
        }
        if (fdPos > 0) {
          parts.push(fdBuf.subarray(0, fdPos));
          totalRead += fdPos;
        }
      }
    }
  }

  if (totalRead === 0) return Buffer.alloc(0);
  if (parts.length === 1) return parts[0];
  return Buffer.concat(parts);
}

// Read a line (up to LF) from leftover buffer, then socket. Bounded by
// __TAIDA_NET_MAX_CHUNK_LINE_BYTES so a streaming chunk-ext flood is treated
// as malformed framing (parity with eager body_complete cap).
// Returns string (synchronous). Throws __NativeError when the cap is hit
// before LF (smuggling vector).
function __taida_net_readLineFromBody(writer) {
  const bs = writer._bodyState;
  const lineParts = [];

  // First drain from leftover.
  while (bs.leftoverPos < bs.leftover.length) {
    if (lineParts.length >= __TAIDA_NET_MAX_CHUNK_LINE_BYTES) {
      throw new __NativeError(
        'chunked body error: chunk-size line exceeds byte cap'
      );
    }
    const b = bs.leftover[bs.leftoverPos];
    bs.leftoverPos++;
    lineParts.push(b);
    if (b === 0x0A) { // LF
      return Buffer.from(lineParts).toString();
    }
  }

  // Then read from socket byte-by-byte until LF.
  while (true) {
    if (lineParts.length >= __TAIDA_NET_MAX_CHUNK_LINE_BYTES) {
      throw new __NativeError(
        'chunked body error: chunk-size line exceeds byte cap'
      );
    }
    const b = __taida_net_readOneByte(writer);
    if (b < 0) break; // EOF
    lineParts.push(b);
    if (b === 0x0A) break;
  }

  return Buffer.from(lineParts).toString();
}

// Drain chunked trailers after terminal chunk. Bounded by line count and
// total trailer-byte length to keep parity with the eager-path cap policy
// in docs/reference/net_api.md §5.4. Both caps trigger __NativeError so the
// caller (readBodyChunk / readBodyAll) aborts the connection rather than
// continuing on keep-alive.
function __taida_net_drainChunkedTrailers(writer) {
  let totalBytes = 0;
  for (let i = 0; i < __TAIDA_NET_MAX_TRAILER_COUNT; i++) {
    const line = __taida_net_readLineFromBody(writer);
    // NB4-18: EOF (0 raw bytes) != valid empty line ("\r\n").
    if (line.length === 0) {
      throw new __NativeError('chunked body error: missing final CRLF after terminal chunk');
    }
    const trimmed = line.trim();
    if (trimmed === '') return;
    totalBytes += trimmed.length;
    if (totalBytes > __TAIDA_NET_MAX_TRAILER_BYTES) {
      throw new __NativeError('chunked body error: trailer block exceeds byte cap');
    }
  }
  // Smuggling guard: more than the count cap of trailer lines without
  // observing the final empty line.
  throw new __NativeError('chunked body error: too many trailer lines');
}

// NET4-3a: readBodyChunk(req) -> Lax[Bytes]
// Reads one chunk from the request body (synchronous from leftover).
function __taida_net_readBodyChunk(req) {
  if (!req || typeof req !== 'object' || req.__body_stream !== '__v4_body_stream') {
    throw new __NativeError(
      'readBodyChunk: can only be called in a 2-argument httpServe handler. ' +
      'In a 1-argument handler, the request body is already fully read. ' +
      'Use readBody(req) instead.'
    );
  }

  const sock = req._socket;
  if (!sock) {
    throw new __NativeError('readBodyChunk: no active socket');
  }

  const writer = sock.__v4_writer;
  if (!writer) {
    throw new __NativeError('readBodyChunk: no active body streaming state');
  }

  // NB4-7: Verify request token.
  if (req.__body_token !== writer._bodyState.requestToken) {
    throw new __NativeError(
      'readBodyChunk: request pack does not match the current active request. ' +
      'The request may be stale or fabricated.'
    );
  }

  if (writer._state === 4) {
    throw new __NativeError('readBodyChunk: cannot read HTTP body after WebSocket upgrade.');
  }

  const bs = writer._bodyState;
  bs.anyReadStarted = true;

  if (bs.fullyRead) {
    return __taida_net_makeLaxBytesEmpty();
  }

  if (bs.isChunked) {
    return __taida_net_readBodyChunkChunkedSync(writer);
  } else {
    return __taida_net_readBodyChunkCLSync(writer);
  }
}

// Chunked TE decode (synchronous from leftover).
function __taida_net_readBodyChunkChunkedSync(writer) {
  const bs = writer._bodyState;

  while (true) {
    switch (bs.chunkedState) {
      case 'done':
        bs.fullyRead = true;
        return __taida_net_makeLaxBytesEmpty();

      case 'waitSize': {
        const line = __taida_net_readLineFromBody(writer);
        // E32B-053: strip only the trailing CRLF terminator, then reject any
        // OWS within chunk-size via the strict hex regex below. RFC 7230 §4.1.
        let stripped = line;
        if (stripped.endsWith('\n')) stripped = stripped.slice(0, -1);
        if (stripped.endsWith('\r')) stripped = stripped.slice(0, -1);
        if (stripped === '') continue; // CRLF-only — try again
        const semi = stripped.indexOf(';');
        const hexStr = semi >= 0 ? stripped.slice(0, semi) : stripped;
        // Strict hex-only parse. Rejects OWS (SP/HT/CR/LF) and partial parse
        // like '1g' uniformly.
        if (!/^[0-9a-fA-F]+$/.test(hexStr)) {
          throw new __NativeError('readBodyChunk: invalid chunk-size \'' + hexStr + '\' in chunked body');
        }
        // Leading-zero policy: 15-digit cap on literal length, parity with
        // Interpreter / Native eager and streaming paths.
        if (hexStr.length > 15) {
          throw new __NativeError('readBodyChunk: invalid chunk-size \'' + hexStr + '\' in chunked body');
        }
        const chunkSize = parseInt(hexStr, 16);
        if (isNaN(chunkSize)) {
          throw new __NativeError('readBodyChunk: invalid chunk-size \'' + hexStr + '\' in chunked body');
        }
        if (chunkSize === 0) {
          bs.chunkedState = 'done';
          bs.fullyRead = true;
          __taida_net_drainChunkedTrailers(writer);
          return __taida_net_makeLaxBytesEmpty();
        }
        bs.chunkedState = 'readData';
        bs.chunkedRemaining = chunkSize;
        break;
      }

      case 'readData': {
        if (bs.chunkedRemaining === 0) {
          bs.chunkedState = 'waitTrailer';
          continue;
        }
        const toRead = Math.min(bs.chunkedRemaining, 8192);
        const data = __taida_net_readBodyBytes(writer, toRead);
        const actuallyRead = data.length;
        // NB4-18: short read (EOF) in chunked data is a protocol error.
        if (actuallyRead === 0) {
          throw new __NativeError(
            'readBodyChunk: truncated chunked body — expected ' +
            bs.chunkedRemaining + ' more chunk-data bytes but got EOF'
          );
        }
        bs.chunkedRemaining -= actuallyRead;
        bs.bytesConsumed += actuallyRead;
        return __taida_net_makeLaxBytesValue(new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
      }

      case 'waitTrailer': {
        // NB4-18: Read CRLF after chunk data and validate.
        const trailerLine = __taida_net_readLineFromBody(writer);
        if (trailerLine.length === 0) {
          throw new __NativeError(
            'readBodyChunk: missing CRLF after chunk data (unexpected EOF)'
          );
        }
        if (trailerLine.trim() !== '') {
          throw new __NativeError(
            'readBodyChunk: malformed chunk trailer — expected CRLF after chunk data, ' +
            'got ' + JSON.stringify(trailerLine)
          );
        }
        bs.chunkedState = 'waitSize';
        break;
      }
    }
  }
}

// Content-Length body decode (synchronous from leftover + socket).
// NB4-18: EOF before Content-Length exhausted is now a protocol error.
function __taida_net_readBodyChunkCLSync(writer) {
  const bs = writer._bodyState;
  const remaining = bs.contentLength - bs.bytesConsumed;
  if (remaining <= 0) {
    bs.fullyRead = true;
    return __taida_net_makeLaxBytesEmpty();
  }
  const toRead = Math.min(remaining, 8192);
  const data = __taida_net_readBodyBytes(writer, toRead);
  if (data.length === 0) {
    // NB4-18: EOF before Content-Length exhausted is a protocol error.
    throw new __NativeError(
      'readBodyChunk: truncated body — expected ' + bs.contentLength +
      ' bytes (Content-Length) but got EOF after ' + bs.bytesConsumed + ' bytes'
    );
  }
  bs.bytesConsumed += data.length;
  if (bs.bytesConsumed >= bs.contentLength) {
    bs.fullyRead = true;
  }
  return __taida_net_makeLaxBytesValue(new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
}

// Lax[Bytes] constructors for readBodyChunk.
function __taida_net_makeLaxBytesEmpty() {
  return Lax(null, new Uint8Array(0));
}

function __taida_net_makeLaxBytesValue(bytes) {
  return Lax(bytes, new Uint8Array(0));
}

// NET4-3a: readBodyAll(req) → Bytes
// Reads all remaining body bytes. This is the only aggregate path.
function __taida_net_readBodyAll(req) {
  if (!req || typeof req !== 'object' || req.__body_stream !== '__v4_body_stream') {
    throw new __NativeError(
      'readBodyAll: can only be called in a 2-argument httpServe handler. ' +
      'In a 1-argument handler, the request body is already fully read. ' +
      'Use readBody(req) instead.'
    );
  }

  const sock = req._socket || (function() {
    throw new __NativeError('readBodyAll: no active socket');
  })();
  const writer = sock.__v4_writer;
  if (!writer) {
    throw new __NativeError('readBodyAll: no active body streaming state');
  }

  // NB4-7: Verify request token.
  if (req.__body_token !== writer._bodyState.requestToken) {
    throw new __NativeError(
      'readBodyAll: request pack does not match the current active request. ' +
      'The request may be stale or fabricated.'
    );
  }

  if (writer._state === 4) {
    throw new __NativeError('readBodyAll: cannot read HTTP body after WebSocket upgrade.');
  }

  const bs = writer._bodyState;
  bs.anyReadStarted = true;

  if (bs.fullyRead) {
    return new Uint8Array(0);
  }

  return __taida_net_readBodyAllImpl(writer);
}

function __taida_net_readBodyAllImpl(writer) {
  const bs = writer._bodyState;
  const allParts = [];
  let totalLen = 0;

  if (bs.isChunked) {
    // Chunked path: read all chunks (synchronous from leftover).
    while (true) {
      switch (bs.chunkedState) {
        case 'done':
          bs.fullyRead = true;
          break;
        case 'waitSize': {
          const line = __taida_net_readLineFromBody(writer);
          // E32B-053: only strip the trailing CRLF terminator, then reject
          // OWS within chunk-size via the strict hex regex below.
          let stripped = line;
          if (stripped.endsWith('\n')) stripped = stripped.slice(0, -1);
          if (stripped.endsWith('\r')) stripped = stripped.slice(0, -1);
          if (stripped === '') continue;
          const semi = stripped.indexOf(';');
          const hexStr = semi >= 0 ? stripped.slice(0, semi) : stripped;
          // Strict hex-only parse (parity with readBodyChunk + eager paths).
          if (!/^[0-9a-fA-F]+$/.test(hexStr)) {
            throw new __NativeError('readBodyAll: invalid chunk-size \'' + hexStr + '\' in chunked body');
          }
          if (hexStr.length > 15) {
            throw new __NativeError('readBodyAll: invalid chunk-size \'' + hexStr + '\' in chunked body');
          }
          const chunkSize = parseInt(hexStr, 16);
          if (isNaN(chunkSize)) {
            throw new __NativeError('readBodyAll: invalid chunk-size \'' + hexStr + '\' in chunked body');
          }
          if (chunkSize === 0) {
            bs.chunkedState = 'done';
            bs.fullyRead = true;
            __taida_net_drainChunkedTrailers(writer);
            break;
          }
          bs.chunkedState = 'readData';
          bs.chunkedRemaining = chunkSize;
          continue;
        }
        case 'readData': {
          if (bs.chunkedRemaining === 0) {
            bs.chunkedState = 'waitTrailer';
            continue;
          }
          const data = __taida_net_readBodyBytes(writer, bs.chunkedRemaining);
          const n = data.length;
          // NB4-18: short read (EOF) in chunked data is a protocol error (parity with readBodyChunk).
          if (n === 0) {
            throw new __NativeError(
              'readBodyAll: truncated chunked body — expected ' +
              bs.chunkedRemaining + ' more chunk-data bytes but got EOF'
            );
          }
          allParts.push(data);
          totalLen += n;
          bs.chunkedRemaining -= n;
          continue;
        }
        case 'waitTrailer': {
          // NB4-18: Read CRLF after chunk data and validate.
          const trailerLine2 = __taida_net_readLineFromBody(writer);
          if (trailerLine2.length === 0) {
            throw new __NativeError(
              'readBodyAll: missing CRLF after chunk data (unexpected EOF)'
            );
          }
          if (trailerLine2.trim() !== '') {
            throw new __NativeError(
              'readBodyAll: malformed chunk trailer — expected CRLF after chunk data, ' +
              'got ' + JSON.stringify(trailerLine2)
            );
          }
          bs.chunkedState = 'waitSize';
          continue;
        }
      }
      if (bs.fullyRead) break;
    }
  } else {
    // Content-Length path: read remaining bytes (synchronous from leftover).
    const remaining = bs.contentLength - bs.bytesConsumed;
    if (remaining > 0) {
      const data = __taida_net_readBodyBytes(writer, remaining);
      bs.bytesConsumed += data.length;
      allParts.push(data);
      totalLen += data.length;
    }
    bs.fullyRead = true;
  }

  // Aggregate (only aggregate path in v4).
  if (allParts.length === 0) return new Uint8Array(0);
  if (allParts.length === 1) return new Uint8Array(allParts[0].buffer, allParts[0].byteOffset, allParts[0].byteLength);
  const result = Buffer.concat(allParts, totalLen);
  return new Uint8Array(result.buffer, result.byteOffset, result.byteLength);
}

// ── v4 WebSocket Implementation ─────────────────────────────

// RFC 6455 magic GUID.
const __WS_GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';
const __WS_MAX_PAYLOAD = 16 * 1024 * 1024; // 16 MiB
const __WS_CONTROL_MAX_PAYLOAD = 125;

function __taida_net_isWsControlOpcode(opcode) {
  return opcode === 0x8 || opcode === 0x9 || opcode === 0xA;
}

function __taida_net_isStrictUtf8(buf) {
  const decoded = buf.toString('utf8');
  const reencoded = Buffer.from(decoded, 'utf8');
  return reencoded.length === buf.length && reencoded.equals(buf);
}

// Compute Sec-WebSocket-Accept from Sec-WebSocket-Key (NET4-3b).
function __taida_net_computeWsAccept(key) {
  if (!__taida_crypto) {
    throw new __NativeError('wsUpgrade: node:crypto module not available');
  }
  const hash = __taida_crypto.createHash('sha1').update(key + __WS_GUID).digest();
  return hash.toString('base64');
}

// Write a WebSocket frame to the socket (NET4-3c).
// Server-to-client: FIN=1, MASK=0.
// Uses synchronous fd writes for plaintext; close frames are written atomically
// so error paths do not tear header and status code apart.
function __taida_net_writeWsFrame(sock, opcode, payload) {
  const payloadLen = payload ? payload.length : 0;
  // Build frame header on stack (max 10 bytes).
  let header;
  if (payloadLen < 126) {
    header = Buffer.alloc(2);
    header[0] = 0x80 | opcode; // FIN=1
    header[1] = payloadLen;    // MASK=0
  } else if (payloadLen <= 65535) {
    header = Buffer.alloc(4);
    header[0] = 0x80 | opcode;
    header[1] = 126;
    header[2] = (payloadLen >> 8) & 0xFF;
    header[3] = payloadLen & 0xFF;
  } else {
    header = Buffer.alloc(10);
    header[0] = 0x80 | opcode;
    header[1] = 127;
    // Write 64-bit big-endian length.
    // JS numbers are safe up to 2^53, sufficient for 16 MiB cap.
    header[2] = 0; header[3] = 0; header[4] = 0; header[5] = 0;
    header[6] = (payloadLen >> 24) & 0xFF;
    header[7] = (payloadLen >> 16) & 0xFF;
    header[8] = (payloadLen >> 8) & 0xFF;
    header[9] = payloadLen & 0xFF;
  }

  // v5: TLS sockets must use socket.write() (decrypted stream API), not raw fd writes.
  // For plaintext, use synchronous fd write to bypass Node's event loop buffering.
  if (sock.__tls) {
    // TLS: use socket stream API (cork/uncork for coalescing).
    sock.cork();
    sock.write(header);
    if (payloadLen > 0) sock.write(payload);
    sock.uncork();
  } else {
    const fd = sock._handle ? sock._handle.fd : -1;
    if (fd >= 0 && __taida_fs) {
      if (opcode === 0x8 && payloadLen > 0) {
        __taida_net_fdWriteAll(fd, Buffer.concat([header, payload]));
      } else {
        __taida_net_fdWriteAll(fd, header);
        if (payloadLen > 0) __taida_net_fdWriteAll(fd, payload);
      }
    } else {
      // Fallback: vectored write via cork/uncork.
      sock.cork();
      sock.write(header);
      if (payloadLen > 0) sock.write(payload);
      sock.uncork();
    }
  }
}

// Synchronous write helper: write all bytes to fd with EAGAIN retry.
function __taida_net_fdWriteAll(fd, buf) {
  let written = 0;
  while (written < buf.length) {
    try {
      const n = __taida_fs.writeSync(fd, buf, written, buf.length - written);
      written += n;
    } catch (e) {
      if (e.code === 'EAGAIN' || e.code === 'EWOULDBLOCK') {
        const spinEnd = Date.now() + 1;
        while (Date.now() < spinEnd) {}
        continue;
      }
      throw new __NativeError('WebSocket write error: ' + (e.message || e));
    }
  }
}

// Read exactly `count` bytes from socket (synchronous).
// Plaintext: uses fs.readSync on the socket fd with EAGAIN retry.
// TLS: uses sock.read() from the internal decrypted buffer (v5 transport boundary).
// The socket must be paused so Node does not consume data from the kernel buffer.
//
// NB5-23: Pre-allocates a single target buffer and copies chunks directly into
// it at the correct offset, avoiding O(n^2) Buffer.concat in the read loop.
function __taida_net_readExactFromSocket(sock, count) {
  if (count === 0) return Buffer.alloc(0);

  // NB5-23: Single allocation for the full result.
  const result = Buffer.alloc(count);
  let pos = 0;

  // First, drain any bytes already in Node's internal read buffer.
  // sock.read() returns data from Node's internal buffer (synchronous).
  while (pos < count) {
    const needed = count - pos;
    const chunk = sock.read(needed);
    if (!chunk) break;
    chunk.copy(result, pos);
    pos += chunk.length;
  }
  if (pos >= count) {
    return result;
  }

  // v5: TLS sockets — use socket.read() polling from the decrypted buffer.
  // Raw fd access is not possible on TLS sockets (would read ciphertext).
  // socket.read() returns decrypted data from Node's internal buffer.
  // We resume the socket briefly to allow TLS data flow, then poll.
  if (sock.__tls) {
    const deadline = Date.now() + 10000; // 10 second timeout
    // Resume the socket so the TLS layer can process incoming data.
    sock.resume();
    while (pos < count) {
      if (Date.now() > deadline) {
        sock.pause();
        throw new __NativeError('wsReceive: timed out waiting for ' + count + ' bytes (got ' + pos + ')');
      }
      const needed = count - pos;
      const chunk = sock.read(needed);
      if (chunk) {
        chunk.copy(result, pos);
        pos += chunk.length;
      } else {
        // Check if socket is closed.
        if (sock.destroyed || !sock.readable) {
          sock.pause();
          throw new __NativeError('wsReceive: connection closed unexpectedly');
        }
        // Busy wait briefly to yield (data arrives asynchronously in TLS layer).
        const spinEnd = Date.now() + 1;
        while (Date.now() < spinEnd) { /* busy wait */ }
      }
    }
    sock.pause();
    return result;
  }

  // Fall back to synchronous fd read for remaining bytes (plaintext only).
  const fd = sock._handle ? sock._handle.fd : -1;
  if (fd < 0 || !__taida_fs) {
    throw new __NativeError('wsReceive: cannot access socket file descriptor for synchronous read');
  }

  // NB5-23: Read directly into the pre-allocated result buffer at the correct offset.
  const remaining = count - pos;
  const deadline = Date.now() + 10000; // 10 second timeout

  while (pos < count) {
    if (Date.now() > deadline) {
      throw new __NativeError('wsReceive: timed out waiting for ' + count + ' bytes (got ' + pos + ')');
    }
    try {
      const n = __taida_fs.readSync(fd, result, pos, count - pos);
      if (n === 0) {
        throw new __NativeError('wsReceive: connection closed unexpectedly');
      }
      pos += n;
    } catch (e) {
      if (e.code === 'EAGAIN' || e.code === 'EWOULDBLOCK') {
        // Spin briefly — data not yet in kernel buffer.
        const spinEnd = Date.now() + 1;
        while (Date.now() < spinEnd) { /* busy wait */ }
        continue;
      }
      throw new __NativeError('wsReceive: read error: ' + (e.message || e));
    }
  }

  return result;
}

// Read and parse one WebSocket frame from the socket (NET4-3c).
// Synchronous — uses readExactFromSocket which does fd-level blocking read.
// Returns {opcode, payload}|{close:true}|{ping:payload}|{pong:true}|{error:msg}
function __taida_net_readWsFrame(sock) {
  // Read first 2 bytes.
  const hdr = __taida_net_readExactFromSocket(sock, 2);
  const byte0 = hdr[0];
  const byte1 = hdr[1];

  const fin = (byte0 & 0x80) !== 0;
  const rsv = byte0 & 0x70;
  const opcode = byte0 & 0x0F;
  const masked = (byte1 & 0x80) !== 0;
  let payloadLen = byte1 & 0x7F;

  // RSV bits must be 0.
  if (rsv !== 0) return { error: 'RSV bits must be 0' };

  // Fragmented frames not supported.
  if (!fin) return { error: 'fragmented frames are not supported' };

  // Continuation opcode without fragmentation is a protocol error.
  if (opcode === 0x0) return { error: 'unexpected continuation frame' };

  // NB4-11: Client-to-server frames MUST be masked (RFC 6455 Section 5.1).
  if (!masked) return { error: 'client frame must be masked (MASK=0 received)' };

  // Extended payload length.
  if (payloadLen === 126) {
    const ext = __taida_net_readExactFromSocket(sock, 2);
    payloadLen = (ext[0] << 8) | ext[1];
  } else if (payloadLen === 127) {
    const ext = __taida_net_readExactFromSocket(sock, 8);
    // Read 64-bit BE. Check MSB = 0.
    if (ext[0] & 0x80) return { error: 'payload length MSB must be 0' };
    payloadLen = 0;
    for (let i = 0; i < 8; i++) payloadLen = payloadLen * 256 + ext[i];
  }

  // Oversized payload check.
  if (payloadLen > __WS_MAX_PAYLOAD) {
    return { error: 'payload too large (' + payloadLen + ' bytes, max ' + __WS_MAX_PAYLOAD + ' bytes)' };
  }

  if (__taida_net_isWsControlOpcode(opcode) && payloadLen > __WS_CONTROL_MAX_PAYLOAD) {
    return { error: 'control frame payload too large (' + payloadLen + ' bytes, max ' + __WS_CONTROL_MAX_PAYLOAD + ' bytes)' };
  }

  // Read masking key.
  let maskKey = null;
  if (masked) {
    maskKey = __taida_net_readExactFromSocket(sock, 4);
  }

  // Read payload.
  let payload = payloadLen > 0
    ? __taida_net_readExactFromSocket(sock, payloadLen)
    : Buffer.alloc(0);

  // NB6-6: Unmask in-place using word-at-a-time XOR via DataView.
  // Process 4 bytes at a time to eliminate modulo per byte.
  if (maskKey) {
    const plen = payload.length;
    const dv = new DataView(payload.buffer, payload.byteOffset, plen);
    const maskWord = (maskKey[0] << 24) | (maskKey[1] << 16) | (maskKey[2] << 8) | maskKey[3];
    let i = 0;
    const wordEnd = plen - 3;
    for (; i < wordEnd; i += 4) {
      dv.setUint32(i, dv.getUint32(i) ^ maskWord);
    }
    for (; i < plen; i++) {
      payload[i] ^= maskKey[i & 3];
    }
  }

  // Dispatch by opcode.
  switch (opcode) {
    case 0x1: // text
    case 0x2: // binary
      return { opcode, payload };
    case 0x8: // close — v5: carry raw payload for close code extraction
      return { close: true, closePayload: payload };
    case 0x9: // ping
      return { ping: payload };
    case 0xA: // pong
      return { pong: true };
    default:
      return { error: 'unknown opcode 0x' + opcode.toString(16).toUpperCase() };
  }
}

// Extract header value from parsed request headers (case-insensitive).
function __taida_net_getHeaderValue(req, targetName) {
  const headers = req.headers;
  const raw = req.raw;
  if (!headers || !raw) return null;
  const lowerTarget = targetName.toLowerCase();
  for (let i = 0; i < headers.length; i++) {
    const h = headers[i];
    if (!h || !h.name) continue;
    // Header name is a span in raw bytes.
    const nStart = h.name.start || 0;
    const nLen = h.name.len || 0;
    const nameStr = Buffer.from(raw.buffer, raw.byteOffset + nStart, nLen).toString().toLowerCase();
    if (nameStr === lowerTarget) {
      const vStart = h.value ? (h.value.start || 0) : 0;
      const vLen = h.value ? (h.value.len || 0) : 0;
      return Buffer.from(raw.buffer, raw.byteOffset + vStart, vLen).toString();
    }
  }
  return null;
}

// Extract method string from parsed request.
function __taida_net_getMethodStr(req) {
  const method = req.method;
  const raw = req.raw;
  if (!method || !raw) return '';
  const start = method.start || 0;
  const len = method.len || 0;
  return Buffer.from(raw.buffer, raw.byteOffset + start, len).toString();
}

// NET4-3b: wsUpgrade(req, writer) → Lax[@(ws: WsConn)]
function __taida_net_wsUpgrade(req, writer) {
  __taida_net_validateWriter(writer, 'wsUpgrade');

  // State check: wsUpgrade only valid in Idle state.
  if (writer._state === 1 || writer._state === 2) {
    throw new __NativeError(
      'wsUpgrade: cannot upgrade after HTTP response has started. ' +
      'wsUpgrade must be called before startResponse/writeChunk.'
    );
  }
  if (writer._state === 3) {
    throw new __NativeError('wsUpgrade: cannot upgrade after HTTP response has ended.');
  }
  if (writer._state === 4) {
    throw new __NativeError('wsUpgrade: WebSocket upgrade already completed.');
  }

  // Must be body-deferred request (2-arg handler).
  if (!req || req.__body_stream !== '__v4_body_stream') {
    return __taida_net_makeLaxWsEmpty();
  }

  // NB4-10: Verify request token matches the active body state.
  if (writer._bodyState && req.__body_token !== writer._bodyState.requestToken) {
    throw new __NativeError(
      'wsUpgrade: request pack does not match the current active request. ' +
      'The request may be stale or fabricated.'
    );
  }

  // NB5-12 (DEFERRED): WebSocket over TLS (wss://) is not supported in the JS
  // backend. This is a documented spec limitation, not a bug.
  //
  // Root cause (PoC verified): Node.js TLS sockets perform decryption via the
  // event loop. In Taida's synchronous handler model, the handler blocks the
  // event loop, so wsReceive's sock.resume()+sock.read() polling never receives
  // decrypted data — sock.read() returns null indefinitely. Plaintext WebSocket
  // works because it uses fs.readSync on the raw fd, bypassing the event loop.
  //
  // Interpreter/Native use rustls which performs synchronous blocking I/O
  // (read ciphertext from TCP stream and decrypt inline), so wss:// works there.
  //
  // Resolution: a future async runtime migration will unblock the event loop during
  // handler execution, enabling TLS WebSocket I/O.
  //
  // Spec refs: NET_DESIGN.md line 343, NET_IMPL_GUIDE.md line 156, NET_BLOCKERS.md NB5-12.
  if (writer._socket && writer._socket.__tls) {
    throw new __NativeError(
      'wsUpgrade: WebSocket over TLS (wss://) is not supported in the JS backend. ' +
      'Node.js TLS requires event-loop I/O which is incompatible with the synchronous ' +
      'handler model. Use plaintext WebSocket (ws://) or the Interpreter/Native backend. ' +
      'This limitation will be resolved with a future async runtime migration.'
    );
  }

  // Validate: must be GET.
  const method = __taida_net_getMethodStr(req);
  if (method.toUpperCase() !== 'GET') {
    return __taida_net_makeLaxWsEmpty();
  }

  // Validate: no body (Content-Length must be 0 or absent, not chunked).
  if ((req.contentLength || 0) > 0 || req.chunked) {
    return __taida_net_makeLaxWsEmpty();
  }

  // Validate: Upgrade: websocket
  const upgradeVal = __taida_net_getHeaderValue(req, 'Upgrade');
  if (!upgradeVal || upgradeVal.toLowerCase() !== 'websocket') {
    return __taida_net_makeLaxWsEmpty();
  }

  // Validate: Connection: Upgrade (may contain comma-separated values)
  const connVal = __taida_net_getHeaderValue(req, 'Connection');
  if (!connVal || !connVal.split(',').some(function(p) { return p.trim().toLowerCase() === 'upgrade'; })) {
    return __taida_net_makeLaxWsEmpty();
  }

  // Validate: Sec-WebSocket-Version: 13
  const versionVal = __taida_net_getHeaderValue(req, 'Sec-WebSocket-Version');
  if (!versionVal || versionVal.trim() !== '13') {
    return __taida_net_makeLaxWsEmpty();
  }

  // NB4-11: Validate Sec-WebSocket-Key (must be 24-char base64, decoding to 16 bytes).
  const wsKey = __taida_net_getHeaderValue(req, 'Sec-WebSocket-Key');
  if (!wsKey || wsKey.trim() === '') {
    return __taida_net_makeLaxWsEmpty();
  }
  // RFC 6455: key must be a base64-encoded 16-byte value (= 24 chars with padding).
  {
    const trimmedKey = wsKey.trim();
    if (trimmedKey.length !== 24 || !/^[A-Za-z0-9+/]{22}==$/.test(trimmedKey)) {
      return __taida_net_makeLaxWsEmpty();
    }
    // Decode and verify 16-byte length.
    try {
      const decoded = Buffer.from(trimmedKey, 'base64');
      if (decoded.length !== 16) {
        return __taida_net_makeLaxWsEmpty();
      }
    } catch (_) {
      return __taida_net_makeLaxWsEmpty();
    }
  }

  // All validations passed. Compute accept and send 101.
  const accept = __taida_net_computeWsAccept(wsKey.trim());
  const response =
    'HTTP/1.1 101 Switching Protocols\r\n' +
    'Upgrade: websocket\r\n' +
    'Connection: Upgrade\r\n' +
    'Sec-WebSocket-Accept: ' + accept + '\r\n' +
    '\r\n';

  const sock = writer._socket;

  // v5: TLS sockets use socket.write() (decrypted stream API).
  // Plaintext: write synchronously via fd to bypass Node's event loop.
  if (sock.__tls) {
    // TLS: use socket.write() — the TLS layer handles encryption transparently.
    sock.write(response);
  } else {
    const fd = sock._handle ? sock._handle.fd : -1;
    if (fd >= 0 && __taida_fs) {
      const respBuf = Buffer.from(response);
      let written = 0;
      while (written < respBuf.length) {
        try {
          const n = __taida_fs.writeSync(fd, respBuf, written, respBuf.length - written);
          written += n;
        } catch (e) {
          if (e.code === 'EAGAIN' || e.code === 'EWOULDBLOCK') {
            const spinEnd = Date.now() + 1;
            while (Date.now() < spinEnd) {}
            continue;
          }
          throw new __NativeError('wsUpgrade: write error: ' + (e.message || e));
        }
      }
    } else {
      sock.write(response);
    }
  }

  // Transition to WebSocket state.
  writer._state = 4; // WebSocket

  // NB4-10: Generate a connection-scoped token for ws identity verification.
  const wsToken = ++__taida_net_wsTokenCounter;
  writer._wsToken = wsToken;

  // Set active ws writer for wsSend/wsReceive/wsClose to find.
  __taida_net_activeWsWriter = writer;

  // Create WsConn pack with identity token.
  const wsPack = Object.freeze({ __ws_id: '__v4_websocket_conn', __ws_token: wsToken });

  return __taida_net_makeLaxWsValue(wsPack);
}

// Lax constructors for WebSocket.
function __taida_net_makeLaxWsEmpty() {
  return Lax(null, Object.freeze({}));
}

function __taida_net_makeLaxWsValue(ws) {
  return Lax(Object.freeze({ ws: ws }), Object.freeze({}));
}

function __taida_net_makeLaxWsFrameValue(typeStr, data) {
  return Lax(Object.freeze({ type: typeStr, data: data }), Object.freeze({}));
}

function __taida_net_makeLaxWsFrameEmpty() {
  return Lax(null, Object.freeze({}));
}

// NB4-10: Validate ws token — checks both sentinel AND connection-scoped token.
function __taida_net_validateWs(ws, apiName) {
  if (!ws || typeof ws !== 'object' || ws.__ws_id !== '__v4_websocket_conn') {
    throw new __NativeError(apiName + ': first argument must be the WebSocket connection from wsUpgrade');
  }
  // Verify connection-scoped token matches the active writer.
  const writer = __taida_net_activeWsWriter;
  if (!writer || ws.__ws_token !== writer._wsToken) {
    throw new __NativeError(
      apiName + ': WebSocket connection does not match the current active connection. ' +
      'The connection may be stale or fabricated.'
    );
  }
}

// Find active writer via ws token's socket reference.
function __taida_net_getWriterForWs(ws, apiName) {
  // The writer is accessible via the socket stored on the writer.
  // Since we don't store back-references on ws, we need to find it.
  // In the JS runtime, the writer is stored on socket.__v4_writer during handler execution.
  // We search through all sockets... but actually we can't easily.
  // Better approach: store a reference on the ws pack itself.
  // Since ws is frozen, we can't add properties. Instead, the ws validation
  // ensures we're in a valid context, and the writer is on the socket.
  // The socket is on the writer.
  // We need a way to get from ws → writer. Let's use a module-level map.
  const writer = __taida_net_activeWsWriter;
  if (!writer) {
    throw new __NativeError(apiName + ': no active WebSocket context');
  }
  return writer;
}

// Module-level reference to the active WebSocket writer.
// Set when wsUpgrade succeeds, cleared when handler completes.
let __taida_net_activeWsWriter = null;

// NB4-10: Monotonic WebSocket connection token counter for identity verification.
let __taida_net_wsTokenCounter = 0;

// NET4-3d: wsSend(ws, data) → Unit
function __taida_net_wsSend(ws, data) {
  __taida_net_validateWs(ws, 'wsSend');
  const writer = __taida_net_getWriterForWs(ws, 'wsSend');

  if (writer._state !== 4) {
    throw new __NativeError('wsSend: not in WebSocket state. Call wsUpgrade first.');
  }
  if (writer._wsClosed) {
    throw new __NativeError('wsSend: WebSocket connection is already closed.');
  }

  const sock = writer._socket;
  let opcode, payload;
  if (typeof data === 'string') {
    opcode = 0x1; // text
    payload = Buffer.from(data, 'utf8');
  } else if (data instanceof Uint8Array) {
    opcode = 0x2; // binary
    payload = data;
  } else {
    throw new __NativeError('wsSend: data must be Str (text frame) or Bytes (binary frame)');
  }

  __taida_net_writeWsFrame(sock, opcode, payload);
  return undefined; // Unit
}

// NET4-3d: wsReceive(ws) → Lax[@(type: Str, data: Bytes|Str)]
// Synchronous — blocks on fd read until a data frame arrives.
function __taida_net_wsReceive(ws) {
  __taida_net_validateWs(ws, 'wsReceive');
  const writer = __taida_net_getWriterForWs(ws, 'wsReceive');

  if (writer._state !== 4) {
    throw new __NativeError('wsReceive: not in WebSocket state. Call wsUpgrade first.');
  }
  if (writer._wsClosed) {
    return __taida_net_makeLaxWsFrameEmpty();
  }

  const sock = writer._socket;

  // Synchronous loop to handle ping/pong transparently.
  while (true) {
    const frame = __taida_net_readWsFrame(sock);

    if (frame.error) {
      // Protocol error: send close frame with 1002.
      __taida_net_writeWsFrame(sock, 0x8, Buffer.from([0x03, 0xEA]));
      writer._wsClosed = true;
      throw new __NativeError('wsReceive: protocol error: ' + frame.error);
    }

    if (frame.close) {
      // v5 close code extraction (NET5-0d).
      const cp = frame.closePayload;
      if (!cp || cp.length === 0) {
        // No status code: reply with empty close payload.
        __taida_net_writeWsFrame(sock, 0x8, Buffer.alloc(0));
        writer._wsClosed = true;
        writer._wsCloseCode = 1005; // No Status Rcvd
        return __taida_net_makeLaxWsFrameEmpty();
      } else if (cp.length === 1) {
        // 1-byte close payload is malformed.
        __taida_net_writeWsFrame(sock, 0x8, Buffer.from([0x03, 0xEA])); // 1002
        writer._wsClosed = true;
        throw new __NativeError('wsReceive: protocol error: malformed close frame (1-byte payload)');
      } else {
        const code = (cp[0] << 8) | cp[1];
        // Validate close code (RFC 6455 Section 7.4).
        // 1000-1003: standard, 1007-1014: IANA-registered, 3000-4999: reserved for libs/apps/private.
        const validCode = (code >= 1000 && code <= 1003) || (code >= 1007 && code <= 1014) || (code >= 3000 && code <= 4999);
        if (!validCode) {
          __taida_net_writeWsFrame(sock, 0x8, Buffer.from([0x03, 0xEA])); // 1002
          writer._wsClosed = true;
          throw new __NativeError('wsReceive: protocol error: invalid close code ' + code);
        }
        // Validate reason UTF-8 if present.
        if (cp.length > 2) {
          try {
            const reason = cp.slice(2);
            if (!__taida_net_isStrictUtf8(reason)) {
              __taida_net_writeWsFrame(sock, 0x8, Buffer.from([0x03, 0xEA])); // 1002
              writer._wsClosed = true;
              throw new __NativeError('wsReceive: protocol error: invalid UTF-8 in close reason');
            }
          } catch (e) {
            if (e instanceof __NativeError) throw e;
            __taida_net_writeWsFrame(sock, 0x8, Buffer.from([0x03, 0xEA])); // 1002
            writer._wsClosed = true;
            throw new __NativeError('wsReceive: protocol error: invalid UTF-8 in close reason');
          }
        }
        // Valid close: echo the code in the reply.
        __taida_net_writeWsFrame(sock, 0x8, Buffer.from([(code >> 8) & 0xFF, code & 0xFF]));
        writer._wsClosed = true;
        writer._wsCloseCode = code;
        return __taida_net_makeLaxWsFrameEmpty();
      }
    }

    if (frame.ping) {
      // Auto pong with same payload.
      __taida_net_writeWsFrame(sock, 0xA, frame.ping);
      continue; // advance to next frame
    }

    if (frame.pong) {
      continue; // unsolicited pong, ignore
    }

    // Data frame (text or binary).
    const typeStr = frame.opcode === 0x1 ? 'text' : 'binary';
    let dataVal;
    if (frame.opcode === 0x1) {
      // Text: return the payload as Str in data field for parity with interpreter.
      if (!__taida_net_isStrictUtf8(frame.payload)) {
        __taida_net_writeWsFrame(sock, 0x8, Buffer.from([0x03, 0xEF])); // 1007
        writer._wsClosed = true;
        throw new __NativeError('wsReceive: invalid UTF-8 in text frame');
      }
      dataVal = frame.payload.toString('utf8');
    } else {
      dataVal = new Uint8Array(frame.payload.buffer, frame.payload.byteOffset, frame.payload.byteLength);
    }
    return __taida_net_makeLaxWsFrameValue(typeStr, dataVal);
  }
}

// NET4-3d: wsClose(ws) → Unit
// v5: wsClose(ws) or wsClose(ws, code) → Unit
function __taida_net_wsClose(ws, code) {
  __taida_net_validateWs(ws, 'wsClose');
  const writer = __taida_net_getWriterForWs(ws, 'wsClose');

  if (writer._state !== 4) {
    throw new __NativeError('wsClose: not in WebSocket state. Call wsUpgrade first.');
  }

  // Idempotent: no-op if already closed.
  if (writer._wsClosed) return undefined;

  // v5: Optional close code (default 1000).
  let closeCode = 1000;
  if (code !== undefined && code !== null) {
    closeCode = code;
    if (typeof closeCode !== 'number' || !Number.isInteger(closeCode)) {
      throw new __NativeError('wsClose: close code must be Int, got ' + String(closeCode));
    }
    if (closeCode < 1000 || closeCode > 4999) {
      throw new __NativeError('wsClose: close code must be 1000-4999, got ' + closeCode);
    }
    // Reserved codes that must not be sent.
    if (closeCode === 1004 || closeCode === 1005 || closeCode === 1006 || closeCode === 1015) {
      throw new __NativeError('wsClose: close code ' + closeCode + ' is reserved and cannot be sent');
    }
  }

  const sock = writer._socket;
  __taida_net_writeWsFrame(sock, 0x8, Buffer.from([(closeCode >> 8) & 0xFF, closeCode & 0xFF]));
  writer._wsClosed = true;

  return undefined; // Unit
}

// v5: wsCloseCode(ws) → Int
function __taida_net_wsCloseCode(ws) {
  __taida_net_validateWs(ws, 'wsCloseCode');
  const writer = __taida_net_getWriterForWs(ws, 'wsCloseCode');

  if (writer._state !== 4) {
    throw new __NativeError('wsCloseCode: not in WebSocket state. Call wsUpgrade first.');
  }

  return writer._wsCloseCode;
}


function isEven(x) {
  if (arguments.length > 1) {
    throw new __TaidaError('ArgumentError', `Function 'isEven' expected at most 1 argument(s), got ${arguments.length}`, {});
  }
  if (arguments.length <= 0) {
    x = __taida_defaultValue('unknown');
  }
  const r = __taida_unmold(Mod_mold(x, 2));
  return __taida_equals(r, 0);
}

function double(x) {
  if (arguments.length > 1) {
    throw new __TaidaError('ArgumentError', `Function 'double' expected at most 1 argument(s), got ${arguments.length}`, {});
  }
  if (arguments.length <= 0) {
    x = __taida_defaultValue('unknown');
  }
  return __taida_mul(x, 2);
}

function lessThan5(x) {
  if (arguments.length > 1) {
    throw new __TaidaError('ArgumentError', `Function 'lessThan5' expected at most 1 argument(s), got ${arguments.length}`, {});
  }
  if (arguments.length <= 0) {
    x = __taida_defaultValue('unknown');
  }
  return (x < 5);
}

function timesTen(x) {
  if (arguments.length > 1) {
    throw new __TaidaError('ArgumentError', `Function 'timesTen' expected at most 1 argument(s), got ${arguments.length}`, {});
  }
  if (arguments.length <= 0) {
    x = __taida_defaultValue('unknown');
  }
  return __taida_mul(x, 10);
}

const s1 = Stream_mold(42);
const r1 = await __taida_unmold_async(s1);
__taida_stdout(__taida_solidify(Join(r1, " ")));
const nums = Object.freeze([1, 2, 3, 4, 5]);
const s2 = StreamFrom(nums);
const r2 = await __taida_unmold_async(s2);
__taida_stdout(__taida_solidify(Join(r2, " ")));
const s3 = StreamFrom(Object.freeze([10, 20, 30]));
const mapped = __taida_solidify(Map(s3, double));
const r3 = await __taida_unmold_async(mapped);
__taida_stdout(__taida_solidify(Join(r3, " ")));
const s4 = StreamFrom(Object.freeze([1, 2, 3, 4, 5, 6]));
const filtered = __taida_solidify(Filter(s4, isEven));
const r4 = await __taida_unmold_async(filtered);
__taida_stdout(__taida_solidify(Join(r4, " ")));
const s5 = StreamFrom(Object.freeze([10, 20, 30, 40, 50]));
const taken = __taida_solidify(Take(s5, 3));
const r5 = await __taida_unmold_async(taken);
__taida_stdout(__taida_solidify(Join(r5, " ")));
const s6 = StreamFrom(Object.freeze([1, 2, 3, 10, 20]));
const tw = __taida_solidify(TakeWhile(s6, lessThan5));
const r6 = await __taida_unmold_async(tw);
__taida_stdout(__taida_solidify(Join(r6, " ")));
const s7 = StreamFrom(Object.freeze([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]));
const step1 = __taida_solidify(Filter(s7, isEven));
const step2 = __taida_solidify(Map(step1, timesTen));
const step3 = __taida_solidify(Take(step2, 3));
const results = await __taida_unmold_async(step3);
__taida_stdout(__taida_solidify(Join(results, " ")));
const s8 = StreamFrom(Object.freeze([1, 2, 3]));
__taida_stdout(__taida_to_string(s8));
const len8 = s8.length_();
__taida_stdout(__taida_to_string(len8));
const empty8 = s8.isEmpty();
__taida_stdout(__taida_to_string(empty8));
const s9 = StreamFrom(Object.freeze([]));
const empty9 = s9.isEmpty();
__taida_stdout(__taida_to_string(empty9));
