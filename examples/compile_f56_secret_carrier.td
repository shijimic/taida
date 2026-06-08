// F56 secret carrier (Level 0): producer + Redact mask.
//
// `MoltenizeSecret[v]()` / `Moltenize[v]()` seal a value in an opaque carrier
// (`Secret[T]` / `Moltenized[T]`). The sealed value can never be displayed,
// serialized, compared, or unmolded directly — only an explicit `Redact[]`
// (fixed "***" mask) or a secret-aware consumer may observe it. This fixture
// pins identical output across interpreter / JS / native / wasm.
secret <= MoltenizeSecret["api-key"]()
stdout(Redact[secret]())

opaque <= Moltenize[42]()
stdout(Redact[opaque]())

// F56 Phase 4: secret-aware consumers. The sealed secret is used (HMAC'd,
// compared in constant time) without ever being revealed to a plain `Str`.
// The MAC (public) and the Bool verdicts are pinned identical across backends.
mac <= HmacSha256[secret, "payload"]()
stdout(mac)
same <= ConstantTimeEq[secret, "api-key"]()
stdout(same.toString())
diff <= ConstantTimeEq[secret, "nope"]()
stdout(diff.toString())
