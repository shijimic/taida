// B-10f mod_a: exports closure, imports mod_b (which imports mod_a back)
>>> ./mod_b.td => @(suffix)

makeGreeter prefix =
  _ name = prefix + ":" + name + suffix()
=> :Str => Str

<<< @(makeGreeter)
