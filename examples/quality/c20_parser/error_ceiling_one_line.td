// C20-1 (ROOT-4 / C19B-008): one-line `|== error: Error = <expr>`
// must load as a distinct top-level statement and not absorb the
// following function definitions or `stdout(...)` calls.
//
// Before the fix `parse_block` used the expression's column as the
// block indent, silently swallowing everything that followed at the
// same indent. The guard for this lives in `parse_error_ceiling`.

Error => ValidationError = @(field: Str, code: Int)

validate text =
  |== error: Error = "caught: " + error.message
  => :Str
  | text == "" |> ValidationError(type <= "V", message <= "Empty", field <= "t", code <= 1).throw()
  | _ |> "Valid: " + text
=> :Str

stdout(validate("hello"))
stdout(validate(""))
