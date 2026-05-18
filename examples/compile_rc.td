// Phase N7: RC テスト
// 多数のヒープ確保 + 関数スコープからの解放パターン

// BuchiPack の確保と使用
p1 <= @(x <= 10, y <= 20)
debug(p1.x)
debug(p1.y)

// リストの確保と使用
nums <= @[1, 2, 3, 4, 5]
debug(Sum[nums]())

// 関数内でのヒープ確保
makePoint x: Int y: Int =
  @(x <= x, y <= y)
=> :@(x: Int, y: Int)

pt <= makePoint(3, 4)
debug(pt.x)
debug(pt.y)

// クロージャ（ヒープ確保される）
multiplier <= 5
scale <= _ x: Int = x * multiplier
debug(scale(6))

// リスト + map
doubled <= Map[@[10, 20, 30], _ x: Int = x * 2]()
debug(Sum[doubled]())
