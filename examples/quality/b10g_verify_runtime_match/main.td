// Quality Test: B-10g — verify vs runtime circular detection consistency
// Uses same circular pattern as B-10a. Run with both `taida verify` and `taida <file>`

>>> ./mod_a.td => @(hello)

stdout(hello("test"))

// Expected: both verify and runtime detect circular import
