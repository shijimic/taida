// RC-6a: Error type inheritance investigation
// Test basic Error inheritance, field composition, multi-level, and throw/|==

// --- Test 1: Basic Error inheritance ---
Error => AppError = @(code: Int)
err1 <= AppError(type <= "AppError", message <= "test error", code <= 42)
Str[err1.code]() >=> code_str
stdout("T1: " + TypeName[err1]() + " code=" + code_str)

// --- Test 2: Field composition (parent message/type + child fields) ---
Error => DetailedError = @(detail: Str, severity: Int)
err2 <= DetailedError(type <= "DetailedError", message <= "something failed", detail <= "db connection", severity <= 3)
Str[err2.severity]() >=> sev_str
stdout("T2: " + err2.type + " msg=" + err2.message + " detail=" + err2.detail + " sev=" + sev_str)

// --- Test 3: Default fields (omit optional child fields) ---
Error => SimpleError = @(tag: Str)
err3 <= SimpleError(type <= "SimpleError", message <= "oops")
stdout("T3: tag=" + err3.tag)

// --- Test 4: throw and |== (basic catch) ---
Error => TestError = @(info: Str)
safeFn x =
  |== error: Error =
    "caught: " + error.message
  => :Str
  TestError(type <= "TestError", message <= "boom", info <= "detail").throw()
  "unreachable"
=> :Str
stdout("T4: " + safeFn(1))

// --- Test 5: throw child, catch as parent Error ---
Error => ChildError = @(reason: Str)
catchParent x =
  |== error: Error =
    "caught(" + error.type + "): " + error.message
  => :Str
  ChildError(type <= "ChildError", message <= "child failed", reason <= "timeout").throw()
  "unreachable"
=> :Str
stdout("T5: " + catchParent(1))

// --- Test 6: Access child-specific fields in catch handler ---
Error => FieldError = @(field: Str)
catchWithFields x =
  |== error: Error =
    "caught: " + error.field
  => :Str
  FieldError(type <= "FieldError", message <= "validation", field <= "email").throw()
  "unreachable"
=> :Str
stdout("T6: " + catchWithFields(1))

// --- Test 7: Multi-level inheritance: Error => Base => Sub ---
Error => BaseError = @(base_info: Str)
BaseError => SubError = @(sub_detail: Str)
sub_err <= SubError(type <= "SubError", message <= "sub failed", base_info <= "base", sub_detail <= "detail")
stdout("T7: " + TypeName[sub_err]() + " base_info=" + sub_err.base_info + " sub_detail=" + sub_err.sub_detail)

// --- Test 8: Multi-level throw and catch ---
Error => Level1Error = @(l1: Str)
Level1Error => Level2Error = @(l2: Str)
catchMulti x =
  |== error: Level2Error =
    "caught(" + error.type + "): " + error.l1 + "+" + error.l2
  => :Str
  Level2Error(type <= "Level2Error", message <= "deep error", l1 <= "one", l2 <= "two").throw()
  "unreachable"
=> :Str
stdout("T8: " + catchMulti(1))
