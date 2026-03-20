// Quality Test: B-8e — モジュール export + 再帰
// export した再帰関数の動作

>>> ./helper.td => @(sumTo)

stdout(sumTo(10, 0).toString())
stdout(sumTo(0, 5).toString())

// Expected: 55
// Expected: 5
