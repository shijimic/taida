// Quality Test: B-8m -- import InheritanceDef with parent fields
// Issue 2B: parent fields and methods should be accessible

>>> ./helper.td => @(Employee)

e <= Employee(name <= "Taro", dept <= "Dev")
stdout(e.name)
stdout(e.dept)
stdout(e.greet())
stdout(e.info())

// Expected: Taro
// Expected: Dev
// Expected: Hi:Taro
// Expected: Taro:Dev
