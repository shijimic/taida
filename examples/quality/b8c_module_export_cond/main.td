// Quality Test: B-8c — モジュール export + パターンマッチ
// export 関数内のパターンマッチが import 元で正常動作

>>> ./helper.td => @(classify)

stdout(classify(200))
stdout(classify(50))
stdout(classify(3))

// Expected: big
// Expected: medium
// Expected: small
