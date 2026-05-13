// D-1m: docs/guide/12_introspection.md code examples
// NOTE: Introspection is mostly CLI-based (taida graph).
// Only testing what can be validated at runtime.

Pilot = @(
  name: Str
  age: Int
  active: Bool
)

asuka <= Pilot(name <= "Asuka", age <= 14, active <= true)
stdout(asuka.name)
stdout(asuka.age.toString())
stdout(asuka.active.toString())

// Validation pattern from doc
Error => ValidationError = @(field: Str)

validatePilot pilot: Pilot =
  | pilot.name == "" |> ValidationError(type <= "ValidationError", message <= "empty name", field <= "name").throw()
  | _ |> pilot.name
=> :Str

validateSafe pilot: Pilot =
  |== error: Error =
    "Error: " + error.message
  => :Str
  | pilot.name == "" |> ValidationError(type <= "ValidationError", message <= "empty name", field <= "name").throw()
  | _ |> pilot.name
=> :Str

stdout(validateSafe(asuka))
empty_pilot <= Pilot(name <= "")
stdout(validateSafe(empty_pilot))
