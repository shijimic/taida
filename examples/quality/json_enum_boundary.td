// C16B-003: Regression coverage for the C16 Lax[Enum] boundary.
//
// Complements `json_enum_validate.td` (the happy-path and basic mismatch
// coverage) with:
//
//   1. Parse-error path — outer Lax `__value` / `__default` contain a TypeDef
//      default whose Enum field must be Int(0), NOT Lax[Enum].
//      (Native regression pin for C16B-001.)
//   2. Success-path outer Lax `__default` also holds pure Int(0) for Enum.
//   3. Lax[Enum].toString() format — 3-backend byte-identical output
//      (C16B-004 pin).
//   4. List[Enum] — each element independently matched / Lax-wrapped.
//   5. List[TypeDef{Enum}] — per-element record with Enum field Lax behaviour.
//   6. Multiple Enum fields in one record with partial failure.
//   7. Variant-name edge cases (digits / underscore / lowercase start).
//   8. Mismatch edge cases (case sensitivity / whitespace / empty / prefix).
//   9. JSON input-type variations against Enum (number / bool / array /
//      object / null).
//
// Interpreter is the reference. Native / JS must produce byte-identical
// output against `json_enum_boundary.expected`.

Enum => Status = :Active :Inactive :Pending
Enum => Priority = :Low :Mid :High
Enum => Level = :Level1 :Level_2 :level3

User = @(name: Str, status: Status)
Ticket = @(status: Status, prio: Priority, note: Str)

// ── 1. Parse-error path — outer Lax.__value.status must be Int(0) ───
JSON["INVALID", User]() => r_bad
stdout(r_bad.has_value.toString())
stdout(r_bad.__value.status.toString())
stdout(r_bad.__default.status.toString())

// ── 2. Success-path — outer Lax.__default.status also Int(0) ─────────
raw_ok <= '{"name":"Alice","status":"Active"}'
JSON[raw_ok, User]() => r_ok
stdout(r_ok.has_value.toString())
stdout(r_ok.__value.status.toString())
stdout(r_ok.__default.status.toString())

// ── 3. Lax[Enum].toString() 3-backend pin ────────────────────────────
raw_bogus <= '{"name":"Dave","status":"Bogus"}'
JSON[raw_bogus, User]() ]=> uBogus
stdout(uBogus.status.toString())

// ── 4. List[Enum] — per-element match / Lax wrap ─────────────────────
raw_list <= '["Active", "Bogus", "Pending", "Inactive"]'
JSON[raw_list, @[Status]]() ]=> items
stdout(items.length().toString())
items.get(0) ]=> e0
stdout(e0.toString())
items.get(1) ]=> e1
stdout(e1.toString())
items.get(2) ]=> e2
stdout(e2.toString())
items.get(3) ]=> e3
stdout(e3.toString())

// ── 5. List[TypeDef{Enum}] — nested Enum Lax per element ─────────────
raw_users <= '[{"name":"A","status":"Active"},{"name":"B","status":"Bogus"},{"name":"C"}]'
JSON[raw_users, @[User]]() ]=> us
stdout(us.length().toString())
us.get(0) ]=> u0
stdout(u0.name)
stdout(u0.status.toString())
us.get(1) ]=> u1
stdout(u1.name)
stdout(u1.status.toString())
us.get(2) ]=> u2
stdout(u2.name)
stdout(u2.status.toString())

// ── 6. Multiple Enum fields, partial failure ─────────────────────────
raw_ticket <= '{"status":"Active","prio":"HighNope","note":"x"}'
JSON[raw_ticket, Ticket]() ]=> t
stdout(t.status.toString())
stdout(t.prio.has_value.toString())
stdout(t.note)

// ── 7. Variant name edge cases ───────────────────────────────────────
JSON['"Level1"', Level]() ]=> lv1
stdout(lv1.toString())
JSON['"Level_2"', Level]() ]=> lv2
stdout(lv2.toString())
JSON['"level3"', Level]() ]=> lv3
stdout(lv3.toString())
JSON['"LEVEL1"', Level]() ]=> lvBad
stdout(lvBad.has_value.toString())

// ── 8. Mismatch edge cases ───────────────────────────────────────────
JSON['"active"', Status]() ]=> mm1
stdout(mm1.has_value.toString())
JSON['" Active"', Status]() ]=> mm2
stdout(mm2.has_value.toString())
JSON['"Active "', Status]() ]=> mm3
stdout(mm3.has_value.toString())
JSON['""', Status]() ]=> mm4
stdout(mm4.has_value.toString())
JSON['"Act"', Status]() ]=> mm5
stdout(mm5.has_value.toString())

// ── 9. JSON input type variations ────────────────────────────────────
JSON['0', Status]() ]=> tv1
stdout(tv1.has_value.toString())
JSON['true', Status]() ]=> tv2
stdout(tv2.has_value.toString())
JSON['["Active"]', Status]() ]=> tv3
stdout(tv3.has_value.toString())
JSON['{"v":"Active"}', Status]() ]=> tv4
stdout(tv4.has_value.toString())
JSON['null', Status]() ]=> tv5
stdout(tv5.has_value.toString())
