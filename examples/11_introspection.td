// Taida Lang Example: Structural Introspection Demo
// Use: taida graph --type dataflow examples/11_introspection.td
// Use: taida graph --type call examples/11_introspection.td
// Use: taida verify examples/11_introspection.td

// Type definitions
Pilot = @(name: Str, age: Int)
Error => ValidationError = @(field: Str)

// Functions
validate name: Str =
  |== error: Error =
    "invalid"
  => :Str

  | name == "" |> ValidationError(type <= "ValidationError", message <= "empty name", field <= "name").throw()
  | _ |> name
=> :Str

greet name: Str =
  valid <= validate(name)
  "Hello, " + valid
=> :Str

// List operations
names <= @["Asuka", "Rei", "Shinji"]
double x: Str =
  x + x
=> :Str

Map[names, double]() >=> doubled
stdout(doubled)

// Entry point
result <= greet("World")
stdout(result)
