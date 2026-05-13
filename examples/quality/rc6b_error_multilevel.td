// RC-6b: Multi-level Error inheritance
// Error => AppError => ValidationError => FieldValidationError

// 3-level chain
Error => AppError = @(app_code: Int)
AppError => ValidationError = @(field_name: Str)
ValidationError => FieldValidationError = @(expected_type: Str)

// Instantiate the deepest child
fve <= FieldValidationError(type <= "FieldValidationError", message <= "invalid type", app_code <= 500, field_name <= "age", expected_type <= "Int")
Str[fve.app_code]() ]=> ac
stdout("T1: " + TypeName[fve]() + " ac=" + ac + " fn=" + fve.field_name + " et=" + fve.expected_type)

// Throw deepest child, catch at root Error level
catchDeep x =
  |== error: Error =
    "caught(" + error.type + "): " + error.message + " field=" + error.field_name
  => :Str
  FieldValidationError(type <= "FieldValidationError", message <= "bad field", app_code <= 400, field_name <= "email", expected_type <= "Str").throw()
  "unreachable"
=> :Str
stdout("T2: " + catchDeep(1))

// Throw middle child, catch at root
catchMiddle x =
  |== error: Error =
    "caught(" + error.type + "): " + error.message
  => :Str
  ValidationError(type <= "ValidationError", message <= "validation failed", app_code <= 422, field_name <= "name").throw()
  "unreachable"
=> :Str
stdout("T3: " + catchMiddle(1))

// Verify field access at each level
AppError => NetworkError = @(url: Str, status: Int)
ne <= NetworkError(type <= "NetworkError", message <= "connection refused", app_code <= 503, url <= "https://api.example.com", status <= 503)
stdout("T4: type=" + ne.type + " msg=" + ne.message)
Str[ne.app_code]() ]=> ne_ac
stdout("T4: app_code=" + ne_ac + " url=" + ne.url)
Str[ne.status]() ]=> ne_st
stdout("T4: status=" + ne_st)
