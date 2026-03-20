// Quality Test: B-8g — モジュール export + TypeDef
// TypeDef 名を直接 export/import してインスタンス化（QF-17 fix）

>>> ./helper.td => @(Dog)

d <= Dog(name <= "Rex", age <= 3)
stdout(d.name)
stdout(d.age.toString())
stdout(d.bark())

// Expected: Rex
// Expected: 3
// Expected: Woof:Rex
