// Quality Test: B-8l -- import TypeDef with alias
// Issue 2A: import alias should work for TypeDef

>>> ./helper.td => @(Dog: PetDog)

p <= PetDog(name <= "Rex")
stdout(p.name)
stdout(p.bark())

// Expected: Rex
// Expected: Woof:Rex
