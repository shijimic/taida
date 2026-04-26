// JSON Molten Iron example
// JSON is opaque — must be cast through a schema: JSON[raw, Schema]()
// Only output-direction functions remain: jsonEncode, jsonPretty

// Define a schema (TypeDef)
Pilot = @(name: Str, age: Int, active: Bool)

// Cast JSON through schema
raw <= '{"name": "Asuka", "age": 14, "active": true}'
JSON[raw, Pilot]() ]=> pilot
stdout("Name: " + pilot.name)
stdout("Age: " + pilot.age.toString())

// Schema handles missing fields (defaults applied)
partial <= '{"name": "Rei"}'
JSON[partial, Pilot]() ]=> pilot2
stdout("Pilot2 age (default): " + pilot2.age.toString())
stdout("Pilot2 active (default): " + pilot2.active.toString())

// Schema handles null (null -> default)
with_null <= '{"name": null, "age": null, "active": null}'
JSON[with_null, Pilot]() ]=> pilot3
stdout("Null name -> empty: '" + pilot3.name + "'")

// Convert Taida value to JSON string (output direction — still works)
json <= jsonEncode(pilot)
stdout("Encoded: " + json)

// Pretty-printed JSON
pretty <= jsonPretty(pilot)
stdout("Pretty:")
stdout(pretty)

// List schema
PilotEntry = @(name: Str, sync_rate: Int)
list_raw <= '[{"name": "Asuka", "syncRate": 78}, {"name": "Rei", "syncRate": 65}]'
JSON[list_raw, @[PilotEntry]]() ]=> pilots
stdout("Number of pilots: " + pilots.length().toString())

// Nested schema
Address = @(city: Str, zip: Str)
Person = @(name: Str, address: Address)
nested_raw <= '{"name": "Asuka", "address": {"city": "Tokyo-3", "zip": "999"}}'
JSON[nested_raw, Person]() ]=> person
stdout("City: " + person.address.city)

// Lax result check
Data = @(x: Int)
result <= JSON['{"x": 42}', Data]()
stdout("Has value: " + result.hasValue.toString())

// Parse error returns Lax with hasValue=false
bad_result <= JSON["not valid json", Data]()
stdout("Bad parse has value: " + bad_result.hasValue.toString())

// Multiple schemas on same data
UserInfo = @(name: Str, age: Int)
StatsInfo = @(score: Float, rank: Int)
multi_raw <= '{"name": "Asuka", "age": 14, "score": 95.5, "rank": 3}'
JSON[multi_raw, UserInfo]() ]=> user
JSON[multi_raw, StatsInfo]() ]=> stats
stdout("User: " + user.name + ", Rank: " + stats.rank.toString())
