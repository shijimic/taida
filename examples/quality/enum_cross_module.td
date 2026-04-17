// C18-1: Enum 型をモジュール越境で使用する parity テスト。
//
// Hachikuma Phase F #1 (E1608) と #6 (関数越境の契約依存) が解消されて
// いることを 3 バックエンドで確認する。variant 順序整合性 [E1618] は
// 別テスト (types/checker_tests.rs の test_c18_1_enum_variant_order_
// mismatch_rejected_e1618) で covered。

>>> ./enum_cross_module_pkg/colors.td => @(Color, pickColor)

// 1. 型越境の happy path: Color:Red() が直接使える。
red <= Color:Red()
stdout(red.toString())

// 2. Enum 同士の等価比較が越境後も成立する。
match <= red == Color:Red()
stdout(match.toString())

// 3. 関数越境の戻り値が import 先で Enum として再利用可能。
picked <= pickColor(1)
isGreen <= picked == Color:Green()
stdout(isGreen.toString())

// 4. 別 variant でも同じ越境経路が期待通り動く (ordinal の健全性)。
blue <= pickColor(99)
stdout(blue.toString())
isBlue <= blue == Color:Blue()
stdout(isBlue.toString())
