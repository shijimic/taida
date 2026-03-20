// B-10e mod_b: imports mod_a (circular!) to use Person typedef
>>> ./mod_a.td => @(Person)

getAge p = p.age => :Int

<<< @(getAge)
