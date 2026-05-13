// Quality Test: D-3e — 型変換モールドの連鎖
// 各型間の変換往復と無効変換の挙動を検証

// Int -> Str -> Int の往復
a <= 42
Str[a]() ]=> s
Int[s]() ]=> i
stdout(i.toString())

// Float -> Int -> Float の往復
d <= 3.14
Int[d]() ]=> i2
Float[i2]() ]=> f2
stdout(f2.toString())

// Bool -> Str -> Bool の往復
g <= true
Str[g]() ]=> s2
stdout(s2)
Bool[s2]() ]=> b2
stdout(b2.toString())

// 連鎖: Int -> Float -> Str -> Int
j <= 123
Float[j]() ]=> fv
Str[fv]() ]=> sv
Int[sv]() ]=> iv
stdout(iv.toString())

// 逆方向: Str -> Int -> Float -> Str
k <= "456"
Int[k]() ]=> ki
Float[ki]() ]=> kf
Str[kf]() ]=> ks
stdout(ks)

// Bool -> Int -> Bool
m <= true
Int[m]() ]=> mi
stdout(mi.toString())
Bool[mi]() ]=> mb
stdout(mb.toString())

// false -> Int -> Bool
n <= false
Int[n]() ]=> ni
stdout(ni.toString())
Bool[ni]() ]=> nb
stdout(nb.toString())

// 無効な変換（Lax の has_value = false）
bad <= Int["abc"]()
stdout(bad.hasValue().toString())
bad ]=> badval
stdout(badval.toString())

// 無効な Float 変換
bad2 <= Float["xyz"]()
stdout(bad2.hasValue().toString())

// 無効な Bool 変換（"maybe" は true/false ではない）
bad3 <= Bool["maybe"]()
stdout(bad3.hasValue().toString())
bad3 ]=> b3val
stdout(b3val.toString())
