// B-3f: TypeDef instantiation inside pattern match arm

Color = @(r: Int, g: Int, b: Int)

pickColor n: Int =
  | n == 1 |> Color(r <= 255, g <= 0, b <= 0)
  | n == 2 |> Color(r <= 0, g <= 255, b <= 0)
  | _ |> Color(r <= 0, g <= 0, b <= 255)
=> :Color

c <= pickColor(2)
stdout(c.r.toString() + "," + c.g.toString() + "," + c.b.toString())

// Expected:
// 0,255,0
