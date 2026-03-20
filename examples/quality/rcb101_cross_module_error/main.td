// Test cross-module error type catching
// ChildErr (defined in error_lib.td) should be caught by |== e: BaseErr
// because ChildErr IS-A BaseErr via inheritance chain.
>>> ./error_lib.td => @(BaseErr, ChildErr, throwChild, throwBase)

// Test 1: ChildErr caught by BaseErr handler (inheritance)
test1 x =
  |== e: BaseErr =
    "caught by BaseErr: " + e.type
  => :Str
  throwChild("child error")
=> :Str

stdout(test1(0))

// Test 2: ChildErr caught by ChildErr handler (exact match)
test2 x =
  |== e: ChildErr =
    "caught by ChildErr: " + e.type
  => :Str
  throwChild("child error 2")
=> :Str

stdout(test2(0))

// Test 3: BaseErr NOT caught by ChildErr handler (parent not caught by child)
test3 x =
  |== e: ChildErr =
    "caught by ChildErr: " + e.type
  => :Str
  |== e: Error =
    "caught by Error: " + e.type
  => :Str
  throwBase("base error")
=> :Str

stdout(test3(0))

// Test 4: Error catches everything (catch-all)
test4 x =
  |== e: Error =
    "caught by Error: " + e.type
  => :Str
  throwChild("any error")
=> :Str

stdout(test4(0))
