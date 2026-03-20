//! Taida JS ランタイム — トランスパイル後の JS に埋め込むヘルパー関数。

pub const RUNTIME_JS: &str = r#"
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

function __taida_castJson(json, schema) {
  if (typeof schema === 'string') {
    switch (schema) {
      case 'Int': return typeof json === 'number' ? Math.trunc(json) : (typeof json === 'string' ? (parseInt(json, 10) || 0) : 0);
      case 'Float': return typeof json === 'number' ? json : (typeof json === 'string' ? (parseFloat(json) || 0.0) : 0.0);
      case 'Str': return typeof json === 'string' ? json : (json === null || json === undefined ? '' : (typeof json === 'object' ? JSON.stringify(json) : String(json)));
      case 'Bool': return typeof json === 'boolean' ? json : false;
      default: {
        // TypeDef lookup
        const td = __taida_typeDefs[schema];
        if (!td || typeof json !== 'object' || json === null || Array.isArray(json)) {
          return __taida_defaultForSchema(schema);
        }
        const result = { __type: schema };
        for (const [fname, fschema] of Object.entries(td)) {
          if (fname in json && json[fname] !== null && json[fname] !== undefined) {
            result[fname] = __taida_castJson(json[fname], fschema);
          } else {
            result[fname] = __taida_defaultForSchema(fschema);
          }
        }
        return Object.freeze(result);
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
        result[fname] = __taida_defaultForSchema(fschema);
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
        if (!td) return '';
        const result = { __type: schema };
        for (const [fname, fschema] of Object.entries(td)) {
          result[fname] = __taida_defaultForSchema(fschema);
        }
        return Object.freeze(result);
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
    if (!/^-?\d+$/.test(value)) return Lax(null, 0);
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
function Replace(str, old, rep, opts) {
  if (typeof str !== 'string') return '';
  if (opts && opts.all) return str.split(old).join(rep);
  return str.replace(old, rep);
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
function __taida_stdout(...args) {
  for (const arg of args) {
    if (__taida_isBytes(arg)) {
      console.log(__taida_bytes_to_string(arg));
    } else if (Array.isArray(arg)) {
      console.log('@[' + arg.map(x => __taida_format(x)).join(', ') + ']');
    } else if (arg && arg.__type === 'Async') {
      const status = arg.status;
      if (status === 'fulfilled') {
        console.log('Async[fulfilled: ' + String(arg.__value) + ']');
      } else if (status === 'rejected') {
        console.log('Async[rejected: ' + String(arg.__error) + ']');
      } else {
        console.log('Async[pending]');
      }
    } else if (arg && arg.__type === 'Result') {
      if (arg.isSuccess()) console.log('Result[' + String(arg.__value) + ']');
      else console.log('Result(throw)');
    } else if (arg && arg.__type === 'Lax') {
      // Match interpreter BuchiPack display format
      const _lhv = typeof arg.hasValue === 'function' ? arg.hasValue() : arg.hasValue;
      console.log('@(hasValue <= ' + String(!!_lhv) + ', __value <= ' + __taida_format(arg.__value) + ', __default <= ' + __taida_format(arg.__default) + ', __type <= "Lax")');
    } else if (arg && typeof arg === 'object' && !Array.isArray(arg)) {
      // BuchiPack-like object
      const entries = Object.entries(arg).filter(([k]) => !k.startsWith('__'));
      const formatted = entries.map(([k, v]) => k + ' <= ' + __taida_format(v)).join(', ');
      console.log('@(' + formatted + ')');
    } else {
      console.log(typeof arg === 'boolean' ? (arg ? 'true' : 'false') : String(arg));
    }
  }
}

function __taida_format(v) {
  if (typeof v === 'string') return '"' + v + '"';
  if (__taida_isBytes(v)) return __taida_bytes_to_string(v);
  if (Array.isArray(v)) return '@[' + v.map(x => __taida_format(x)).join(', ') + ']';
  if (typeof v === 'boolean') return v ? 'true' : 'false';
  if (v && typeof v === 'object' && !Array.isArray(v) && !v.__type) {
    const entries = Object.entries(v).filter(([k]) => !k.startsWith('__'));
    return '@(' + entries.map(([k, val]) => k + ' <= ' + __taida_format(val)).join(', ') + ')';
  }
  return String(v);
}

// ── stderr — Taida error output function (prelude) ──────
function __taida_stderr(...args) {
  for (const a of args) process.stderr.write(String(a) + '\n');
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

// ── taida-lang/os — Core-bundled OS package (13 APIs) ──
// Uses Node.js fs, child_process, process modules.

// ESM: reuse __taida_fs for fs operations, load child_process via dynamic import
const __os_fs = __taida_fs || null;
const __os_cp = await import('node:child_process').catch(() => null);

const __OS_MAX_READ_SIZE = 64 * 1024 * 1024; // 64 MB

// Helper: create os Result success value
function __taida_os_result_ok(inner) {
  return __taida_result_create(inner, null, null);
}

// Helper: build IoError value from runtime error object
function __taida_os_io_error(err) {
  const code = err && err.errno !== undefined ? err.errno : -1;
  const message = err && err.message ? err.message : String(err);
  return { __type: 'IoError', type: 'IoError', message: message, fields: { code: code } };
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
  return __taida_result_create(inner, __taida_os_io_error(err), null);
}

// Helper: create os Result failure with explicit kind/message (non-OS errors)
function __taida_os_result_fail_with_kind(kind, message) {
  const inner = Object.freeze({ ok: false, code: -1, message: message, kind: kind });
  const errVal = { __type: 'IoError', type: 'IoError', message: message, fields: { code: -1, kind: kind } };
  return __taida_result_create(inner, errVal, null);
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

// ── Input molds (Read, ListDir, Stat, Exists, EnvVar) ──

function __taida_os_read(path) {
  if (!__os_fs) return Lax(null, '');
  try {
    const stat = __os_fs.statSync(path);
    if (stat.size > __OS_MAX_READ_SIZE) return Lax(null, '');
    const content = __os_fs.readFileSync(path, 'utf-8');
    return Lax(content);
  } catch (e) {
    return Lax(null, '');
  }
}

function __taida_os_readBytes(path) {
  if (!__os_fs) return __taida_lax_from_bytes(new Uint8Array(0), false);
  try {
    const stat = __os_fs.statSync(path);
    if (stat.size > __OS_MAX_READ_SIZE) return __taida_lax_from_bytes(new Uint8Array(0), false);
    const content = __os_fs.readFileSync(path);
    return __taida_lax_from_bytes(new Uint8Array(content), true);
  } catch (e) {
    return __taida_lax_from_bytes(new Uint8Array(0), false);
  }
}

function __taida_os_listdir(path) {
  if (!__os_fs) return Lax(null, Object.freeze([]));
  try {
    const entries = __os_fs.readdirSync(path).sort();
    return Lax(Object.freeze(entries));
  } catch (e) {
    return Lax(null, Object.freeze([]));
  }
}

function __taida_os_stat(path) {
  const defaultStat = Object.freeze({ size: 0, modified: '', isDir: false });
  if (!__os_fs) return Lax(null, defaultStat);
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
    return Lax(null, defaultStat);
  }
}

function __taida_os_exists(path) {
  if (!__os_fs) return false;
  return __os_fs.existsSync(path);
}

function __taida_os_envvar(name) {
  const val = typeof process !== 'undefined' && process.env ? process.env[name] : undefined;
  if (val !== undefined) return Lax(val);
  return Lax(null, '');
}

// ── Side-effect functions (writeFile, appendFile, remove, createDir, rename) ──

function __taida_os_writeFile(path, content) {
  try {
    __os_fs.writeFileSync(path, content);
    return __taida_os_result_ok(__taida_os_ok_inner());
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_writeBytes(path, content) {
  try {
    const payload = __taida_to_bytes_payload(content);
    __os_fs.writeFileSync(path, payload);
    return __taida_os_result_ok(__taida_os_ok_inner());
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_appendFile(path, content) {
  try {
    __os_fs.appendFileSync(path, content);
    return __taida_os_result_ok(__taida_os_ok_inner());
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_remove(path) {
  try {
    __os_fs.rmSync(path, { recursive: true, force: false });
    return __taida_os_result_ok(__taida_os_ok_inner());
  } catch (e) {
    return __taida_os_result_fail(e);
  }
}

function __taida_os_createDir(path) {
  try {
    __os_fs.mkdirSync(path, { recursive: true });
    return __taida_os_result_ok(__taida_os_ok_inner());
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
      const throwVal = { __type: 'ProcessError', type: 'ProcessError', message: "Process '" + program + "' exited with code " + code, fields: { code: code } };
      return __taida_os_gorillax_fail(throwVal);
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
      const throwVal = { __type: 'ProcessError', type: 'ProcessError', message: 'Shell command exited with code ' + code + ': ' + command, fields: { code: code } };
      return __taida_os_gorillax_fail(throwVal);
    }
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
  return Lax(null, Object.freeze({ status: 0, body: '', headers: Object.freeze({}) }));
}

// ReadAsync[path]() -> Promise<Lax[Str]>
async function __taida_os_readAsync(path) {
  if (!__os_fs) return Lax(null, '');
  try {
    const fsp = __os_fs.promises || await import('node:fs/promises').then(m => m.default || m).catch(() => null);
    if (!fsp) return Lax(null, '');
    const stat = await fsp.stat(path);
    if (stat.size > 64 * 1024 * 1024) return Lax(null, '');
    const content = await fsp.readFile(path, 'utf-8');
    return Lax(content);
  } catch (e) {
    return Lax(null, '');
  }
}

// HttpGet[url]() -> Promise<Lax[@(status, body, headers)]>
async function __taida_os_httpGet(url) {
  try {
    const resp = await fetch(url);
    const body = await resp.text();
    const headers = [];
    resp.headers.forEach((v, k) => headers.push([k, v]));
    return __taida_os_http_response(resp.status, body, headers);
  } catch (e) {
    return __taida_os_http_failure();
  }
}

// HttpPost[url, body]() -> Promise<Lax[@(status, body, headers)]>
async function __taida_os_httpPost(url, body) {
  try {
    const resp = await fetch(url, { method: 'POST', body: body || '' });
    const respBody = await resp.text();
    const headers = [];
    resp.headers.forEach((v, k) => headers.push([k, v]));
    return __taida_os_http_response(resp.status, respBody, headers);
  } catch (e) {
    return __taida_os_http_failure();
  }
}

// HttpRequest[method, url](headers, body) -> Promise<Lax[@(status, body, headers)]>
async function __taida_os_httpRequest(method, url, reqHeaders, body) {
  try {
    const opts = { method: method || 'GET' };
    if (body) opts.body = body;
    if (reqHeaders && typeof reqHeaders === 'object') {
      const h = {};
      for (const [k, v] of Object.entries(reqHeaders)) {
        if (typeof v === 'string') h[k] = v;
      }
      if (Object.keys(h).length > 0) opts.headers = h;
    }
    const resp = await fetch(url, opts);
    const respBody = await resp.text();
    const headers = [];
    resp.headers.forEach((v, k) => headers.push([k, v]));
    return __taida_os_http_response(resp.status, respBody, headers);
  } catch (e) {
    return __taida_os_http_failure();
  }
}

// TCP: load net module
const __os_net = await import('node:net').catch(() => null);
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
  if (!socket || !socket.once) return Lax(null, '');
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
      finish(Lax(null, ''));
    };
    const onError = () => {
      finish(Lax(null, ''));
    };
    const timer = setTimeout(() => {
      finish(Lax(null, ''));
    }, effectiveTimeout);

    socket.once('data', onData);
    socket.once('end', onEnd);
    socket.once('error', onError);
  });
}

// socketRecvBytes(socket, timeoutMs?) -> Promise<Lax[Bytes]>
async function __taida_os_socketRecvBytes(socketOrPack, timeoutMs) {
  const socket = (socketOrPack && socketOrPack.socket) ? socketOrPack.socket : socketOrPack;
  if (!socket || !socket.once) return __taida_lax_from_bytes(new Uint8Array(0), false);
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
      finish(__taida_lax_from_bytes(new Uint8Array(0), false));
    };
    const onError = () => {
      finish(__taida_lax_from_bytes(new Uint8Array(0), false));
    };
    const timer = setTimeout(() => {
      finish(__taida_lax_from_bytes(new Uint8Array(0), false));
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
  const defaultPayload = Object.freeze({ host: '', port: 0, data: new Uint8Array(0), truncated: false });
  if (!socket || typeof socket.once !== 'function') return Lax(null, defaultPayload);
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
    const onError = () => {
      finish(Lax(null, defaultPayload));
    };
    const timer = setTimeout(() => {
      finish(Lax(null, defaultPayload));
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
  if (!socket || !socket.once) return __taida_lax_from_bytes(new Uint8Array(0), false);
  if (!__taida_isIntNumber(size) || size < 0) return __taida_lax_from_bytes(new Uint8Array(0), false);
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
    const onError = () => finish(__taida_lax_from_bytes(new Uint8Array(0), false));
    const onEnd = () => finish(__taida_lax_from_bytes(new Uint8Array(0), false));
    const timer = setTimeout(() => finish(__taida_lax_from_bytes(new Uint8Array(0), false)), effectiveTimeout);
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
    hasValue: __taida_hasValue(!!hasValue),
    isEmpty() { return !hasValue; },
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
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// FL-28: Verify stdin does not use /dev/stdin (Windows incompatible)
    #[test]
    fn test_stdin_no_dev_stdin_hardcode() {
        assert!(
            !RUNTIME_JS.contains("'/dev/stdin'"),
            "JS runtime should not hardcode '/dev/stdin' -- use process.stdin.fd for cross-platform compatibility"
        );
        assert!(
            !RUNTIME_JS.contains("\"/dev/stdin\""),
            "JS runtime should not hardcode \"/dev/stdin\""
        );
        // Verify the cross-platform approach is used
        assert!(
            RUNTIME_JS.contains("process.stdin.fd"),
            "JS runtime should use process.stdin.fd for cross-platform stdin"
        );
    }
}
