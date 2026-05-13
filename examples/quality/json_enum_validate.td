// C16: JSON[raw, schema]() must validate Enum-typed fields.
// On variant match → ordinal.
// On mismatch / missing / null → Lax[Enum] (silent coercion 禁止).

Enum => Status = :Active :Inactive :Pending

User = @(name: Str, status: Status)

// ── Case 1: Variant match returns ordinal ───────────────
raw1 <= '{"name": "Alice", "status": "Active"}'
JSON[raw1, User]() ]=> u1
stdout(u1.name)
stdout(u1.status.toString())

raw1b <= '{"name": "Beatrice", "status": "Inactive"}'
JSON[raw1b, User]() ]=> u1b
stdout(u1b.name)
stdout(u1b.status.toString())

raw1c <= '{"name": "Carol", "status": "Pending"}'
JSON[raw1c, User]() ]=> u1c
stdout(u1c.name)
stdout(u1c.status.toString())

// ── Case 2: Invalid variant Str → Lax[Enum] ─────────────
raw2 <= '{"name": "Dave", "status": "Bogus"}'
JSON[raw2, User]() ]=> u2
stdout(u2.name)
stdout(u2.status.has_value.toString())
stdout(u2.status.getOrDefault(2).toString())

// ── Case 3: Missing Enum key → Lax[Enum] ────────────────
raw3 <= '{"name": "Eve"}'
JSON[raw3, User]() ]=> u3
stdout(u3.name)
stdout(u3.status.has_value.toString())
stdout(u3.status.getOrDefault(1).toString())

// ── Case 4: null Enum value → Lax[Enum] ─────────────────
raw4 <= '{"name": "Frank", "status": null}'
JSON[raw4, User]() ]=> u4
stdout(u4.name)
stdout(u4.status.has_value.toString())

// ── Case 5: Top-level Enum schema ───────────────────────
raw5 <= '"Pending"'
JSON[raw5, Status]() ]=> s5
stdout(s5.toString())

raw5b <= '"NotReal"'
JSON[raw5b, Status]() ]=> s5b
stdout(s5b.has_value.toString())
