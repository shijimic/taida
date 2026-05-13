// D-1e: docs/guide/04_buchi_pack.md code examples

// Value creation
pack <= @(name <= "Asuka", age <= 14, active <= true)
stdout(pack.name)
stdout(pack.age.toString())

// Type definition
Pilot = @(
  name: Str
  age: Int
  active: Bool
)
rei <= Pilot(name <= "Rei", age <= 14, active <= true)
stdout(rei.name)

// Field access
pilot <= @(name <= "Shinji", age <= 14, call_sign <= "Ikari")
pilot_name <= pilot.name
pilot_age <= pilot.age
pilot_call <= pilot.call_sign
stdout(pilot_name)
stdout(pilot_age.toString())
stdout(pilot_call)

// Default values for typed fields
Pilot2 = @(name: Str, call_sign: Str, age: Int)
rei2 <= Pilot2(name <= "Rei")
stdout(rei2.call_sign)
stdout(rei2.age.toString())

// Inheritance
Pilot3 = @(
  name: Str
  age: Int
)
Pilot3 => NervStaff = @(
  department: Str
  rank: Int
)
staff <= NervStaff(name <= "Ritsuko", age <= 30, department <= "Science", rank <= 2)
stdout(staff.name)
stdout(staff.department)

// Nested buchi pack
pilot4 <= @(
  name <= "Rei",
  contact <= @(
    email <= "rei@nerv.jp",
    phone <= "NERV-001"
  )
)
email <= pilot4.contact.email
stdout(email)

// Nested type definition
Pilot5 = @(
  name: Str
  age: Int
  base: @(
    city: Str
    zip: Str
    org: @(
      code: Str
      name: Str
    )
  )
)
pilot5 <= Pilot5(name <= "Shinji", age <= 14, base <= @(city <= "Tokyo-3", zip <= "999-0003", org <= @(code <= "NERV", name <= "NERV HQ")))
org_code <= pilot5.base.org.code
stdout(org_code)

// Functions in buchi pack (via TypeDef - inline form doesn't parse, QF finding)
MathUtils = @(
  add x y = x + y => :Int
  subtract x y = x - y => :Int
  pi: Float
)
math_utils <= MathUtils(pi <= 3.14159)
result <= math_utils.add(3, 5)
stdout(result.toString())
stdout(math_utils.pi.toString())

// Mold in buchi pack
pilot_data <= @(
  name <= "Asuka",
  sync_rate <= Div[780, 10]()
)
pilot_data.sync_rate ]=> rateValue
stdout(rateValue.toString())

// List literal
numbers <= @[1, 2, 3, 4, 5]
names <= @["Asuka", "Shinji", "Rei"]
stdout(numbers.length().toString())
stdout(names)

// JSON conversion
Pilot6 = @(name: Str, age: Int)
raw <= '{"name": "Ritsuko", "age": 30}'
JSON[raw, Pilot6]() ]=> pilot6
stdout(pilot6.name)
stdout(pilot6.age.toString())
