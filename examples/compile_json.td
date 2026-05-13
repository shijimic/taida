// JSON Molten Iron — native compile test
// Tests JSON[raw, Schema]() schema-based casting with all 6 rules

// Define a schema
Person = @(name: Str, age: Int)

// 1. Basic: Cast JSON through schema -> Lax, unmold to get BuchiPack
raw <= '{"name":"Asuka","age":14}'
JSON[raw, Person]() ]=> person
stdout("Name: " + person.name)
stdout("Age: " + person.age.toString())

// 2. Missing fields get defaults (Rule 2)
partial <= '{"name":"Rei"}'
JSON[partial, Person]() ]=> person2
stdout("Person2 age (default): " + person2.age.toString())

// 3. Null -> defaults (Rule 4)
with_null <= '{"name":null,"age":null}'
JSON[with_null, Person]() ]=> person3
stdout("Null name -> empty: '" + person3.name + "'")

// 4. Primitive schema: Str
JSON['"hello"', Str]() ]=> greeting
stdout("Greeting: " + greeting)

// 5. Primitive schema: Int
JSON['42', Int]() ]=> num
stdout("Num: " + num.toString())

// 6. Nested schema (Rule 5: recursive matching)
Address = @(city: Str, zip: Str)
UserAddr = @(name: Str, address: Address)
nested_raw <= '{"name":"Asuka","address":{"city":"Tokyo-3","zip":"999"}}'
JSON[nested_raw, UserAddr]() ]=> user
stdout("City: " + user.address.city)
stdout("Zip: " + user.address.zip)

// 7. Extra fields ignored (Rule 1: only schema fields extracted)
ExtraRaw <= '{"name":"Asuka","age":14,"extra":"ignored","bonus":99}'
JSON[ExtraRaw, Person]() ]=> trimmed
stdout("Trimmed name: " + trimmed.name)
stdout("Trimmed age: " + trimmed.age.toString())

// 8. Type mismatch -> default (Rule 3)
TypeMismatch = @(age: Int, name: Str)
mismatch_raw <= '{"age":"not a number","name":42}'
JSON[mismatch_raw, TypeMismatch]() ]=> mm
stdout("Mismatch age: " + mm.age.toString())

// 9. Multiple schemas on same data
UserInfo = @(name: Str, age: Int)
StatsInfo = @(score: Int, rank: Int)
multi_raw <= '{"name":"Asuka","age":14,"score":95,"rank":3}'
JSON[multi_raw, UserInfo]() ]=> u2
JSON[multi_raw, StatsInfo]() ]=> s2
stdout("User: " + u2.name + ", Rank: " + s2.rank.toString())
