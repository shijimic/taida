// D-2f: docs/reference/scope_rules.md code examples

// Module scope
global_name <= "Module Level"
stdout(global_name)

// Function scope
processData =
  local_var <= "Function Level"
  local_var
=> :Str

stdout(processData())

// Nested scope via functions
outer x: Int =
  inner y: Int =
    x + y
  => :Int
  inner(10)
=> :Int

stdout(outer(5).toString())

// Scope-based auto-management
createPilot name: Str =
  temp <= @(name <= name, processed <= true)
  @(name <= temp.name, active <= temp.processed)
=> :@(name: Str, active: Bool)

pilot <= createPilot("Rei")
stdout(pilot.name)
stdout(pilot.active.toString())
