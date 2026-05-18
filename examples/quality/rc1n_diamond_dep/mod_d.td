// RC-1n: Diamond dependency — shared leaf module D
// Both B and C import this module. D should be inlined only once.

_secret x: Str = "[" + x + "]" => :Str

shared x: Str = "D:" + _secret(x) => :Str

<<< @(shared)
