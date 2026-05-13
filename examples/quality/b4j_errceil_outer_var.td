// Quality Test: B-4j — エラー天井 + 外部変数
// |== 本体内から外側関数の引数・ローカル変数を参照（F-49 回帰）

Error => MyError = @(code: Int)
global_tag <= "G"

outer arg =
  local <= arg + 100
  |== error: Error =
    global_tag + ":" + arg.toString() + ":" + local.toString() + ":" + error.message
  => :Str
  | arg < 0 |> MyError(type <= "MyError", message <= "neg", code <= 1).throw()
  | _ |> "ok:" + arg.toString()
=> :Str

stdout(outer(5))
stdout(outer(-1))

// Expected:
// ok:5
// G:-1:99:neg
