// Quality Test: B-8b — モジュール export ラムダ（BuchiPack 経由）
// export した BuchiPack 内ラムダの呼び出し

>>> ./helper.td => @(ops)

stdout(ops.add(3, 4).toString())
stdout(ops.mul(5, 6).toString())

// Expected: 7
// Expected: 30
