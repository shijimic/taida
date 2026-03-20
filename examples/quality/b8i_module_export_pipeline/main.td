// Quality Test: B-8i — モジュール export + パイプライン
// export 関数内のパイプラインが正常動作

>>> ./helper.td => @(processNums)

data <= @[1, 2, 3, 4, 5, 6]
stdout(processNums(data).toString())

// Expected: 120
