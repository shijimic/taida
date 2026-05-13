Pilot = @(name: Str, age: Int, role: Str)

pilot_member <= Pilot(name <= _, age <= 14, role <= "pilot")
rei <= pilot_member("Rei")
stdout(rei.name)
stdout(rei.age.toString())
stdout(rei.role)
