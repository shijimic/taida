// C18-1: Enum 型をモジュール越境で公開する exporter 側。
// importer は `>>> ./enum_cross_module_pkg/colors.td => @(Color, pickColor)`
// で Color 型と pickColor 関数を同時に利用できる。

Enum => Color = :Red :Green :Blue

// Color を引数に取り Enum を返す関数（#6 契約依存テストの pin 用）。
pickColor n =
  | n == 0 |> Color:Red()
  | n == 1 |> Color:Green()
  | _ |> Color:Blue()
=> :Color

<<< @(Color, pickColor)
