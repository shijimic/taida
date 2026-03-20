// Quality Test: C-1e — BuchiPack フィールド関数がモジュール内の別関数を再帰呼び出し（F-56 + B-6d 複合）
>>> ./helper.td => @(tools)

stdout(tools.run(3))
stdout(tools.run(1))
// Expected:
// 3,2,1,
// 1,
