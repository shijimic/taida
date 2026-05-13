trim_it <= Trim[_]()
stdout(trim_it("  hi  "))

split_comma <= Split[_, ","]()
parts <= split_comma("a,b,c")
stdout(parts.length().toString())
