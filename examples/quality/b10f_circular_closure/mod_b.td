// B-10f mod_b: imports mod_a (circular!) to reference closure
>>> ./mod_a.td => @(makeGreeter)

suffix = "!" => :Str

<<< @(suffix)
