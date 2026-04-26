// D28B-015: strOf(span, raw) function-form basic case
raw <= "hello world"
span <= @(start <= 6, len <= 5)
result <= strOf(span, raw)
stdout(result)
