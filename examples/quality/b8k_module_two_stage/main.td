// Quality Test: B-8k — 2 段モジュール import
// main -> mod_a -> mod_b で mod_b の関数を mod_a の closure が参照

>>> ./mod_a.td => @(sumSquares)

data <= @[1, 2, 3, 4]
stdout(sumSquares(data).toString())

// Expected: 30
