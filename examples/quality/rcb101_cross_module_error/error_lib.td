// Library module defining custom error types
Error => BaseErr = @()
BaseErr => ChildErr = @()

throwChild msg =
  ChildErr(type <= "ChildErr", message <= msg).throw()
  ""
=> :Str

throwBase msg =
  BaseErr(type <= "BaseErr", message <= msg).throw()
  ""
=> :Str

<<< @(BaseErr, ChildErr, throwChild, throwBase)
