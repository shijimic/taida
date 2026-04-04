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

// ── taida-lang/net: HTTP v1 runtime ─────────────────────────────

// Helper: create net Result success (reuses __taida_result_create)
function __taida_net_result_ok(inner) {
  return __taida_result_create(inner, null, null);
}

// Helper: create net Result failure with kind/message
function __taida_net_result_fail(kind, message) {
  const inner = Object.freeze({ ok: false, code: -1, message: message, kind: kind });
  const errVal = { __type: 'HttpError', type: 'HttpError', message: message, fields: { kind: kind } };
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
    // NB-7: Enforce header name/value length limits in UTF-8 bytes (parity with Interpreter/Native)
    if (Buffer.byteLength(name, 'utf-8') > 8192) {
      return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: headers[' + i + '].name exceeds 8192 bytes');
    }
    if (Buffer.byteLength(value, 'utf-8') > 65536) {
      return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: headers[' + i + '].value exceeds 65536 bytes');
    }
    // Reject CRLF in header name/value
    if (name.includes('\r') || name.includes('\n')) {
      return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: headers[' + i + '].name contains CR/LF');
    }
    if (value.includes('\r') || value.includes('\n')) {
      return __taida_net_result_fail('EncodeError', 'httpEncodeResponse: headers[' + i + '].value contains CR/LF');
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
    // NB-7: Enforce header name/value length limits (parity with public encoder)
    if (Buffer.byteLength(name, 'utf-8') > 8192) return null;
    if (Buffer.byteLength(value, 'utf-8') > 65536) return null;
    // Reject CRLF in header name/value to prevent response splitting
    if (name.includes('\r') || name.includes('\n')) return null;
    if (value.includes('\r') || value.includes('\n')) return null;
    if (noBody && name.toLowerCase() === 'content-length') continue;
    head += name + ': ' + value + '\r\n';
    if (name.toLowerCase() === 'content-length') hasContentLength = true;
  }
  if (!noBody && !hasContentLength) {
    head += 'Content-Length: ' + bodyBytes.length + '\r\n';
  }
  head += '\r\n';
  return { head: Buffer.from(head, 'latin1'), body: bodyBytes };
}

// NET2-4b: Chunked Transfer Encoding in-place compaction (JS)
// Mirrors Interpreter's chunked_in_place_compact algorithm.
// buf is a Buffer; bodyOffset is where chunk framing starts.
// Returns { bodyLen, wireConsumed } on success, or null on malformed input.
function __taida_net_chunkedInPlaceCompact(buf, bodyOffset) {
  const dataLen = buf.length - bodyOffset;
  let readPos = 0;
  let writePos = 0;

  function findCRLF(start) {
    for (let i = start; i < dataLen - 1; i++) {
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
    // Trim ASCII whitespace from hex part
    let hexStart = readPos;
    while (hexStart < hexEnd && (buf[bodyOffset + hexStart] === 0x20 || buf[bodyOffset + hexStart] === 0x09)) hexStart++;
    while (hexEnd > hexStart && (buf[bodyOffset + hexEnd - 1] === 0x20 || buf[bodyOffset + hexEnd - 1] === 0x09)) hexEnd--;

    if (hexStart >= hexEnd) return null; // empty chunk-size

    const hexStr = buf.toString('latin1', bodyOffset + hexStart, bodyOffset + hexEnd);
    if (!/^[0-9a-fA-F]+$/.test(hexStr)) return null; // strict hex validation
    // NB2-4: Reject oversized chunk-size (parity with body_complete)
    if (hexStr.length > 15) return null; // malformed: oversized chunk-size
    const chunkSize = parseInt(hexStr, 16);
    if (isNaN(chunkSize) || chunkSize < 0 || !Number.isSafeInteger(chunkSize)) return null; // invalid hex

    // Advance past "chunk-size\r\n"
    readPos = crlfPos + 2;

    // Terminator chunk (size == 0)
    if (chunkSize === 0) {
      // Skip optional trailer headers until final CRLF
      for (;;) {
        if (readPos + 2 > dataLen) return null; // malformed: missing final CRLF
        if (buf[bodyOffset + readPos] === 13 && buf[bodyOffset + readPos + 1] === 10) {
          readPos += 2;
          return { bodyLen: writePos, wireConsumed: readPos };
        }
        // Skip trailer line
        const trlf = findCRLF(readPos);
        if (trlf < 0) return null; // malformed: incomplete trailer
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

    // Find CRLF after chunk-size
    let crlfPos = -1;
    for (let i = readPos; i < dataLen - 1; i++) {
      if (buf[bodyOffset + i] === 13 && buf[bodyOffset + i + 1] === 10) { crlfPos = i; break; }
    }
    if (crlfPos < 0) return -1; // need more data

    // Parse chunk-size hex
    let hexEnd = crlfPos;
    for (let i = readPos; i < crlfPos; i++) {
      if (buf[bodyOffset + i] === 0x3B) { hexEnd = i; break; }
    }
    let hexStart = readPos;
    while (hexStart < hexEnd && (buf[bodyOffset + hexStart] === 0x20 || buf[bodyOffset + hexStart] === 0x09)) hexStart++;
    while (hexEnd > hexStart && (buf[bodyOffset + hexEnd - 1] === 0x20 || buf[bodyOffset + hexEnd - 1] === 0x09)) hexEnd--;
    if (hexStart >= hexEnd) return -2; // malformed: empty chunk-size

    const hexStr = buf.toString('latin1', bodyOffset + hexStart, bodyOffset + hexEnd);
    if (!/^[0-9a-fA-F]+$/.test(hexStr)) return -2; // strict hex validation
    // NB2-4: Reject oversized chunk-size that would exceed safe integer range.
    // Prevents JS parseInt wrapping to Infinity / imprecise float for huge hex values.
    if (hexStr.length > 15) return -2; // malformed: oversized chunk-size
    const chunkSize = parseInt(hexStr, 16);
    if (isNaN(chunkSize) || chunkSize < 0 || !Number.isSafeInteger(chunkSize)) return -2; // malformed

    readPos = crlfPos + 2;

    if (chunkSize === 0) {
      // Skip trailers
      for (;;) {
        if (readPos + 2 > dataLen) return -1;
        if (buf[bodyOffset + readPos] === 13 && buf[bodyOffset + readPos + 1] === 10) {
          return readPos + 2; // complete
        }
        let trlf = -1;
        for (let i = readPos; i < dataLen - 1; i++) {
          if (buf[bodyOffset + i] === 13 && buf[bodyOffset + i + 1] === 10) { trlf = i; break; }
        }
        if (trlf < 0) return -1;
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
      } else {
        return new __TaidaAsync(
          __taida_net_result_fail('ProtocolError',
            'httpServe: protocol must be a Str, got ' + typeof tls.protocol),
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

// Validate that headers don't contain reserved names for streaming path.
function __taida_net_validateReservedHeaders(headers) {
  for (let i = 0; i < headers.length; i++) {
    const name = (headers[i].name || '').toLowerCase();
    if (name === 'content-length') {
      throw new __NativeError(
        "startResponse: 'Content-Length' is not allowed in streaming response headers. " +
        'The runtime manages Content-Length/Transfer-Encoding for streaming responses.');
    }
    if (name === 'transfer-encoding') {
      throw new __NativeError(
        "startResponse: 'Transfer-Encoding' is not allowed in streaming response headers. " +
        'The runtime manages Transfer-Encoding for streaming responses.');
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
  const h = Array.isArray(headers) ? headers : [];

  // Validate reserved headers
  __taida_net_validateReservedHeaders(h);

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

// Read a line (up to LF) from leftover buffer, then socket.
// Returns string (synchronous).
function __taida_net_readLineFromBody(writer) {
  const bs = writer._bodyState;
  const lineParts = [];

  // First drain from leftover.
  while (bs.leftoverPos < bs.leftover.length) {
    const b = bs.leftover[bs.leftoverPos];
    bs.leftoverPos++;
    lineParts.push(b);
    if (b === 0x0A) { // LF
      return Buffer.from(lineParts).toString();
    }
  }

  // Then read from socket byte-by-byte until LF.
  while (true) {
    const b = __taida_net_readOneByte(writer);
    if (b < 0) break; // EOF
    lineParts.push(b);
    if (b === 0x0A) break;
  }

  return Buffer.from(lineParts).toString();
}

// Drain chunked trailers after terminal chunk (NB4-8 parity).
function __taida_net_drainChunkedTrailers(writer) {
  for (let i = 0; i < 64; i++) {
    const line = __taida_net_readLineFromBody(writer);
    // NB4-18: EOF (0 raw bytes) != valid empty line ("\r\n").
    if (line.length === 0) {
      throw new __NativeError('chunked body error: missing final CRLF after terminal chunk');
    }
    if (line.trim() === '') return;
  }
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
        const trimmed = line.trim();
        if (trimmed === '') continue;
        const hexStr = trimmed.split(';')[0].trim();
        // NB4-18: Strict hex-only parse. Reject partial parse like '1g'.
        if (!/^[0-9a-fA-F]+$/.test(hexStr)) {
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
  return Object.freeze({
    hasValue: __taida_hasValue(false),
    __value: new Uint8Array(0),
    __default: new Uint8Array(0),
    __type: 'Lax',
  });
}

function __taida_net_makeLaxBytesValue(bytes) {
  return Object.freeze({
    hasValue: __taida_hasValue(true),
    __value: bytes,
    __default: new Uint8Array(0),
    __type: 'Lax',
  });
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
          const trimmed = line.trim();
          if (trimmed === '') continue;
          const hexStr = trimmed.split(';')[0].trim();
          // NB4-18: Strict hex-only parse (parity with readBodyChunk).
          if (!/^[0-9a-fA-F]+$/.test(hexStr)) {
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
// Uses cork/uncork to coalesce header + payload (no Buffer.concat).
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
      __taida_net_fdWriteAll(fd, header);
      if (payloadLen > 0) __taida_net_fdWriteAll(fd, payload);
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
  return Object.freeze({
    hasValue: __taida_hasValue(false),
    __value: Object.freeze({}),
    __default: Object.freeze({}),
    __type: 'Lax',
  });
}

function __taida_net_makeLaxWsValue(ws) {
  return Object.freeze({
    hasValue: __taida_hasValue(true),
    __value: Object.freeze({ ws: ws }),
    __default: Object.freeze({}),
    __type: 'Lax',
  });
}

function __taida_net_makeLaxWsFrameValue(typeStr, data) {
  return Object.freeze({
    hasValue: __taida_hasValue(true),
    __value: Object.freeze({ type: typeStr, data: data }),
    __default: Object.freeze({}),
    __type: 'Lax',
  });
}

function __taida_net_makeLaxWsFrameEmpty() {
  return Object.freeze({
    hasValue: __taida_hasValue(false),
    __value: Object.freeze({}),
    __default: Object.freeze({}),
    __type: 'Lax',
  });
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
            // Check for invalid UTF-8 sequences by round-tripping.
            const decoded = reason.toString('utf8');
            const reencoded = Buffer.from(decoded, 'utf8');
            if (reencoded.length !== reason.length || !reencoded.equals(reason)) {
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

    /// Regression: JS sseEvent must NOT build a single aggregate payload string.
    /// The old code concatenated everything into `ssePayload` before writing,
    /// defeating the zero-copy streaming design. The fix writes each SSE line
    /// separately within cork/uncork.
    #[test]
    fn test_sse_event_no_aggregate_payload_string() {
        // The old pattern was: let ssePayload = ''; ssePayload += ...
        // followed by sock.write(ssePayload). After the fix, sseEvent should
        // write 'event: ' and 'data: ' lines directly to the socket.
        assert!(
            !RUNTIME_JS.contains("let ssePayload = ''"),
            "JS sseEvent should not build an aggregate ssePayload string — \
             write SSE lines directly to socket within cork/uncork"
        );
        assert!(
            !RUNTIME_JS.contains("sock.write(ssePayload)"),
            "JS sseEvent should not write a single aggregate ssePayload — \
             write each SSE line separately"
        );
    }

    /// Regression: JS writeChunk must track drain (backpressure) state.
    /// sock.write() returns false when the kernel buffer is full; the writer
    /// must record this so callers can react to backpressure.
    #[test]
    fn test_write_chunk_tracks_drain() {
        assert!(
            RUNTIME_JS.contains("_needsDrain"),
            "JS writer should have a _needsDrain flag for backpressure tracking"
        );
        assert!(
            RUNTIME_JS.contains("'drain'"),
            "JS runtime should listen for 'drain' events to clear backpressure flag"
        );
    }

    /// Regression: JS drain listener must not accumulate across keep-alive
    /// requests.  The old code added `socket.on('drain', ...)` per request
    /// but never removed it, causing listener leak on keep-alive connections.
    /// The fix removes drain listeners in afterResponseWritten alongside
    /// timeout/end/error cleanup.
    #[test]
    fn test_drain_listener_cleaned_up_between_requests() {
        // afterResponseWritten must remove drain listeners before re-attaching
        // for the next request (same pattern as timeout/end/error).
        assert!(
            RUNTIME_JS.contains("removeAllListeners('drain')"),
            "afterResponseWritten should remove drain listeners to prevent \
             accumulation on keep-alive connections"
        );
    }

    /// Regression: JS drain listener must store a removable reference.
    /// The old code used an anonymous function in `socket.on('drain', ...)`,
    /// making targeted removal impossible.  The fix stashes the handler
    /// as `writer._onDrain` for per-listener removal, and also uses
    /// `removeAllListeners('drain')` for bulk cleanup.
    #[test]
    fn test_drain_listener_has_named_reference() {
        assert!(
            RUNTIME_JS.contains("_onDrain"),
            "drain listener should be stashed as _onDrain for removability"
        );
    }

    /// Contract: writeChunk/sseEvent are synchronous Unit functions.
    /// They must NOT return a Promise on backpressure — that would break
    /// the public contract (NET_DESIGN) and backend parity.
    /// Instead they only set `_needsDrain = true` for observability and
    /// always return undefined (Unit).
    #[test]
    fn test_backpressure_is_sync_unit() {
        // writeChunk/sseEvent must set _needsDrain flag on backpressure
        assert!(
            RUNTIME_JS.contains("writer._needsDrain = true;"),
            "writeChunk/sseEvent should set _needsDrain flag on backpressure"
        );
        // Must NOT return a Promise (drain Promise would break sync Unit contract)
        assert!(
            !RUNTIME_JS.contains("return new Promise(function(resolve) { writer._drainResolve"),
            "writeChunk/sseEvent must NOT return a drain Promise — \
             they are synchronous Unit per NET_DESIGN contract"
        );
        // Must NOT have _drainResolve field (no async drain resolution)
        assert!(
            !RUNTIME_JS.contains("_drainResolve"),
            "writer must NOT have _drainResolve field — \
             backpressure is handled by Node.js internal buffering"
        );
    }
}
