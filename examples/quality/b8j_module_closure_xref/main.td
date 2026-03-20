// Quality Test: B-8j — モジュール内 closure 相互参照
// export 関数 A の closure が非 export 関数 B を参照（F-56 回帰）

>>> ./helper.td => @(wrapAll)

data <= @[1, 2, 3]
stdout(wrapAll(data))

// Expected: [1],[2],[3]
