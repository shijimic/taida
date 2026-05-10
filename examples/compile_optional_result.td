// Lax[T] / Result[T, P] method tests for native compiler
// (Optional abolished in v0.8.0, replaced by Lax)

// === Lax tests (replaces Optional) ===

// Test 1: Lax[value]() creates Lax with value
opt <= Lax[42]()
debug(opt.hasValue())

// Test 2: Div[0,0]() creates empty Lax
empty <= Div[0, 0]()
debug(empty.hasValue())

// Test 3: Lax.isEmpty()
debug(Lax[1]().isEmpty())
debug(Div[0, 0]().isEmpty())

// Test 4: Lax.getOrDefault()
debug(Lax[99]().getOrDefault(0))
debug(Div[0, 0]().getOrDefault(777))

// Test 5: Lax.map(fn)
double x = x * 2 => :Int
mapped <= Lax[21]().map(double)
debug(mapped.getOrDefault(0))

// Test 6: Empty Lax.map(fn) stays empty
none_mapped <= Div[0, 0]().map(double)
debug(none_mapped.getOrDefault(0))

// Test 7: Lax.flatMap(fn)
wrapDouble x: Int = Lax[x * 2]() => :Lax[Int]
flat_mapped <= Lax[10]().flatMap(wrapDouble)
debug(flat_mapped.getOrDefault(0))

// Test 8: Empty Lax.flatMap stays empty
none_flat <= Div[0, 0]().flatMap(wrapDouble)
debug(none_flat.getOrDefault(0))

// === Result tests ===

// Test 9: Result[value]() creates successful Result
ok_val <= Result[100]()
debug(ok_val.isSuccess())

// Test 10: Result with throw creates failed Result
Error => TestError = @(message: Str)
err_val <= Result[0](throw <= TestError(message <= "error"))
debug(err_val.isError())

// Test 11: Result.getOrDefault()
debug(Result[55]().getOrDefault(0))
debug(Result[0](throw <= TestError(message <= "fail")).getOrDefault(0))

// Test 12: Result.map(fn) on success
ok_mapped <= Result[25]().map(double)
debug(ok_mapped.getOrDefault(0))

// Test 13: Result.map(fn) on error stays error
err_mapped <= Result[0](throw <= TestError(message <= "fail")).map(double)
debug(err_mapped.isError())

// Test 14: Result.flatMap(fn)
wrapResult x: Int = Result[x * 3]() => :Result[Int, TestError]
flat_ok <= Result[7]().flatMap(wrapResult)
debug(flat_ok.getOrDefault(0))

// Test 15: Error Result.flatMap stays error
flat_err <= Result[0](throw <= TestError(message <= "fail")).flatMap(wrapResult)
debug(flat_err.isError())

// === Lax[T] method tests ===

// Test 16: Lax.map(fn)
lax_val <= Div[100, 4]()
lax_mapped <= lax_val.map(double)
lax_mapped ]=> lm_result
debug(lm_result)

// Test 17: Empty Lax.map stays empty
lax_empty <= Div[100, 0]()
lax_em <= lax_empty.map(double)
lax_em ]=> le_result
debug(le_result)

// Test 18: Lax.flatMap(fn)
wrapLax x: Int = Lax[x * 5]() => :Lax[Int]
lax_fm <= Div[10, 2]().flatMap(wrapLax)
lax_fm ]=> lfm_result
debug(lfm_result)

// === Result[T, P] predicate tests ===

// Test 19: Result[value, _ = true]() ]=> value — predicate passes
Result[42, _ = true]() ]=> pred_ok
debug(pred_ok)

// Test 20: Result[value, _ x = x >= 18]() — validation predicate
debug(Result[25, _ x = x >= 18]().isSuccess())
debug(Result[15, _ x = x >= 18]().isSuccess())

// Test 21: getOrDefault with predicate
debug(Result[42, _ = true]().getOrDefault(0))
debug(Result[42, _ = false]().getOrDefault(0))

// Test 22: map with predicate
debug(Result[21, _ = true]().map(double).getOrDefault(0))
debug(Result[21, _ = false]().map(double).isError())

// Test 23: backward compatibility — no predicate
Result[42]() ]=> bc_val
debug(bc_val)
