// compile_b11_2f_stdout.td — B11-2f regression fixture
// Phase 2 の退行を防ぐ: stdout の非 Bool 経路で Str/MoldInst が正しく出力されることを検証する。
// 3 backend (interp / JS / native) + 3 wasm profile (min / wasi / full) で同一出力が期待される。

// --- 空文字列 ---
stdout("")
stdout("after-empty")

// --- Str 返却モールド（インラインで stdout に渡す） ---
lst <= @[1, 2, 3]
stdout(Join[lst, ","]())
stdout(Join[@[5], " "]())
stdout(Join[@[], "-"]())

// --- 他の Str 返却モールドを変数束縛なしで ---
stdout(Upper["hi"]())
stdout(Lower["HELLO"]())
stdout(Trim["  pad  "]())
stdout(Repeat["ab", 3]())

// --- 混在パイプライン（現場でよく出るパターン） ---
msg <= "Hello, World"
stdout(msg)
stdout(Upper[msg]())
stdout("---done---")
