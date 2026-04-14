// B11-2d: Bool stdout display parity test (FB-3)
// Verifies that Bool values display as "true"/"false" across all 3 backends.

// Direct Bool literals
stdout(true)
stdout(false)

// Bool variables
x <= true
y <= false
stdout(x)
stdout(y)

// Comparison operators (return Bool)
stdout(1 == 1)
stdout(1 > 2)
stdout(3 >= 3)
stdout(5 != 5)

// Boolean operators
stdout(true && true)
stdout(true && false)
stdout(false || true)
stdout(false || false)
stdout(!true)
stdout(!false)

// Bool in BuchiPack fields (typed)
Config = @(debug: Bool, verbose: Bool)
config <= Config(debug <= false, verbose <= true)
stdout(config.debug)
stdout(config.verbose)

// Bool in anonymous BuchiPack fields (inferred from default value)
pack <= @(active <= true, hidden <= false)
stdout(pack.active)
stdout(pack.hidden)
