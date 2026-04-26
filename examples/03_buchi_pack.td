// === Taida Lang: Buchi Pack ===
pilot <= @(name <= "Asuka", age <= 14, call_sign <= "Eva-02")

stdout(`Name: ${pilot.name}`)
stdout(`Age: ${pilot.age}`)
stdout(`CallSign: ${pilot.call_sign}`)

// Nested buchi pack
staff <= @(
  name <= "Rei",
  contact <= @(
    email <= "rei@nerv.jp",
    phone <= "NERV-001"
  )
)

stdout(`${staff.name}'s email: ${staff.contact.email}`)
stdout(`${staff.name}'s phone: ${staff.contact.phone}`)
