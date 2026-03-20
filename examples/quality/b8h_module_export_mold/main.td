// Quality Test: B-8h — モジュール export + モールド + unmold
// export 関数がモールドを返す → import 元で unmold

>>> ./helper.td => @(safeDivide)

stdout(safeDivide(10, 2).toString())
stdout(safeDivide(10, 0).toString())

// Expected: 5
// Expected: 0
