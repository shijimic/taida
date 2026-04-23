// C25B-025 Phase 5-A: Sqrt and Pow produce correct values on
// interpreter and JS. Previously, neither was implemented in the
// interpreter and Sqrt[4.0]() silently returned a BuchiPack
// wrapper instead of 2.0.
stdout(Sqrt[4.0]())
stdout(Sqrt[2.0]())
stdout(Pow[2.0, 10]())
stdout(Pow[3.0, 2]())
