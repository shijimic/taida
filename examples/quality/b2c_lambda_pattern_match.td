// B-2c: ラムダ内パターンマッチ
// CondBranch 後の外側変数アクセス（F-46 回帰）

prefix <= "Result"

// C20-5: multi-line rhs multi-arm guard is rejected with [E0303]; wrap in parens.
classify x =
  label <= (
    | x > 0 |> "positive"
    | x == 0 |> "zero"
    | _ |> "negative"
  )
  prefix + ":" + label
=> :Str

applier <= _ val = classify(val)

stdout(applier(5))
stdout(applier(0))
stdout(applier(0 - 1))

// Expected:
// Result:positive
// Result:zero
// Result:negative
