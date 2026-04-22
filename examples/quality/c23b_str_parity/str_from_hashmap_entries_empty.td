// C23B-009 (2026-04-22): empty HashMap entries() produces `@[]`.
// This case was already correct on all four backends (empty list short-
// circuits before field-name lookup), but we pin it so a future regression
// of the list allocation path doesn't silently break the edge case.
m <= hashMap()
stdout(Str[m.entries()]())
