// C23B-007 nested audit: a mixed list containing a nested mixed list
// must render correctly at both levels. The outer list is
// HETEROGENEOUS (String + List) so the outer elem_type_tag latches
// to -2; the inner list is HETEROGENEOUS (Int + String) and does the
// same. Both levels fall back to the structural debug-string helper
// which recursively handles nested lists / strings.
stdout(Str[@[1, @["a", 2]]]())
