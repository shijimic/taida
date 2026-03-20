// Quality Test: B-8a — モジュール export 関数
// export した関数を import 元から呼び出し

>>> ./helper.td => @(double, greet)

stdout(double(21).toString())
stdout(greet("Taida"))

// Expected: 42
// Expected: Hello:Taida
