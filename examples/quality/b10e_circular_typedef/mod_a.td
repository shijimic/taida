// B-10e mod_a: exports TypeDef, imports mod_b (which imports mod_a back)
>>> ./mod_b.td => @(getAge)

Person = @(name: Str, age: Int)

<<< @(Person)
