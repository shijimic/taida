// === Taida Lang: Prelude — Result ===

// Result は操作モールド: 成功値と Error 継承型の throw payload を一緒に運ぶ
Error => NotFound = @(message: Str)

success <= Result[42]()
failure <= Result[0](throw <= NotFound(message <= "not found"))

// メソッド: isSuccess / isError
stdout("success.isSuccess() = " + success.isSuccess().toString())
stdout("failure.isError() = " + failure.isError().toString())

// メソッド: getOrDefault
stdout("success.getOrDefault(0) = " + success.getOrDefault(0).toString())
stdout("failure.getOrDefault(0) = " + failure.getOrDefault(0).toString())

// メソッド: map
double x: Int = x * 2 => :Int
stdout("Result[21]().map(double) = " + Result[21]().map(double).getOrDefault(0).toString())

Error => Fail = @(message: Str)
stdout("Result[0](throw).map(double).isError() = " + Result[0](throw <= Fail(message <= "fail")).map(double).isError().toString())

// メソッド: mapError — mapper には throw payload (Fail) がそのまま渡される
addPrefix e: Fail = "Error: " + e.message => :Str
stdout("failure.mapError(addPrefix) = " + Result[0](throw <= Fail(message <= "fail")).mapError(addPrefix).toString())

// メソッド: flatMap
safeDivide x: Int =
  | x > 0 |> Result[Div[100, x]().unmold()]()
  | _ |> Result[0]()
=> :Result[Int, ErrorInfo]

stdout("Result[5]().flatMap(safeDivide) = " + Result[5]().flatMap(safeDivide).toString())
stdout("Result[0]().flatMap(safeDivide) = " + Result[0]().flatMap(safeDivide).toString())

// メソッド: toString
stdout("Result[42]().toString() = " + Result[42]().toString())
stdout("failure.toString() = " + failure.toString())
