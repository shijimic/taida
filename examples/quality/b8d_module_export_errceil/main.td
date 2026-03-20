// Quality Test: B-8d — モジュール export + エラー天井
// export 関数内の |== が import 元で正常動作

>>> ./helper.td => @(safeParse)

stdout(safeParse("hello"))
stdout(safeParse(""))

// Expected: ok:hello
// Expected: error:empty
