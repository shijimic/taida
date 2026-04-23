// C25B-025 Phase 5-A: transcendentals (Sin / Cos / Tan / Exp / Ln /
// Log2 / Log10 / Asin / Acos / Atan / Atan2 / Sinh / Cosh / Tanh).
// Pre-fix, none of these were registered in `mold_returns.rs` so
// referencing them raised a type error; with Phase 5-A they are
// Float-returning molds on both interpreter and JS.
stdout(Sin[0.0]())
stdout(Cos[0.0]())
stdout(Exp[1.0]())
stdout(Ln[2.718281828459045]())
stdout(Log10[100.0]())
stdout(Log2[8.0]())
stdout(Atan2[1.0, 1.0]())
stdout(Log[8.0, 2.0]())
