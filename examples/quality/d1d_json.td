// D-1d: docs/guide/03_json.md code examples

// Basic JSON schema
Pilot = @(name: Str, age: Int, active: Bool)
raw <= '{"name": "Asuka", "age": 14, "active": true}'
JSON[raw, Pilot]() ]=> pilot
stdout(pilot.name)
stdout(pilot.age.toString())
stdout(pilot.active.toString())

// Nested schema
Address = @(city: Str, zip: Str)
PilotNested = @(name: Str, age: Int, address: Address)
raw2 <= '{"name": "Asuka", "age": 14, "address": {"city": "Tokyo-3", "zip": "999-0003"}}'
JSON[raw2, PilotNested]() ]=> pilot2
stdout(pilot2.address.city)
stdout(pilot2.address.zip)

// List schema
PilotSync = @(name: Str, sync_rate: Int)
raw3 <= '[{"name": "Asuka", "sync_rate": 95}, {"name": "Shinji", "sync_rate": 41}]'
JSON[raw3, @[PilotSync]]() ]=> pilots
stdout(pilots.length().toString())
pilots.get(0) ]=> p0
stdout(p0.name)
pilots.get(1) ]=> p1
stdout(p1.name)

// Rule 1: Extra fields ignored
Pilot2 = @(name: Str, age: Int)
raw4 <= '{"name": "Asuka", "age": 14, "extra": "ignored"}'
JSON[raw4, Pilot2]() ]=> pilot4
stdout(pilot4.name)
stdout(pilot4.age.toString())

// Rule 2: Missing field -> default
Pilot3 = @(name: Str, age: Int, active: Bool)
raw5 <= '{"name": "Rei"}'
JSON[raw5, Pilot3]() ]=> pilot5
stdout(pilot5.name)
stdout(pilot5.age.toString())
stdout(pilot5.active.toString())

// Rule 3: Type mismatch -> default
Pilot4 = @(name: Str, age: Int)
raw6 <= '{"name": "Asuka", "age": "not a number"}'
JSON[raw6, Pilot4]() ]=> pilot6
stdout(pilot6.name)
stdout(pilot6.age.toString())

// Rule 4: null -> default
Pilot5 = @(name: Str, age: Int, active: Bool)
raw7 <= '{"name": "Rei", "age": null, "active": null}'
JSON[raw7, Pilot5]() ]=> pilot7
stdout(pilot7.name)
stdout(pilot7.age.toString())
stdout(pilot7.active.toString())

// Rule 5: Nested recursive matching
Address2 = @(city: Str, zip: Str)
Pilot6 = @(name: Str, address: Address2)
raw8 <= '{"name": "Asuka", "address": {"city": "Tokyo-3"}}'
JSON[raw8, Pilot6]() ]=> pilot8
stdout(pilot8.address.city)
stdout(pilot8.address.zip)

// Rule 6: List per-element matching
Pilot7 = @(name: Str, sync_rate: Int)
raw9 <= '[{"name": "Rei", "sync_rate": 95}, {"name": "Shinji"}]'
JSON[raw9, @[Pilot7]]() ]=> pilots2
pilots2.get(0) ]=> p2_0
stdout(p2_0.name)
stdout(p2_0.sync_rate.toString())
pilots2.get(1) ]=> p2_1
stdout(p2_1.name)
stdout(p2_1.sync_rate.toString())

// Parse failure -> hasValue false
JSON["invalid json", Pilot]().has_value => has_val
stdout(has_val.toString())

// jsonEncode output
pilot_out <= @(name <= "Asuka", age <= 14, active <= true)
stdout(jsonEncode(pilot_out))
