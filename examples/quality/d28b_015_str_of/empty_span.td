// D28B-015: empty span -> empty Str
raw <= "hello"
span <= @(start <= 0, len <= 0)
result <= strOf(span, raw)
stdout(`[${result}]`)
