// models.td -- Domain types for inventory system
//
// Demonstrates: type inheritance (custom + error), field composition

// --- Product type hierarchy (custom inheritance) ---
Product = @(name: Str, price: Int)
Product => StockedItem = @(qty: Int)
StockedItem => PerishableItem = @(days_left: Int)

// --- Error type hierarchy (error inheritance) ---
Error => InventoryError = @(reason: Str)
InventoryError => StockError = @(item: Str)

// --- Factory functions ---
mkStocked name price qty =
  StockedItem(name <= name, price <= price, qty <= qty)

mkPerishable name price qty days_left =
  PerishableItem(name <= name, price <= price, qty <= qty, days_left <= days_left)

// --- Domain predicates ---
totalValue item =
  item.price * item.qty
=> :Int

isLowStock item =
  item.qty < 5
=> :Bool

isExpensive item =
  item.price > 100
=> :Bool

isExpiring item =
  item.days_left < 3
=> :Bool

<<< @(Product, StockedItem, PerishableItem, InventoryError, StockError, mkStocked, mkPerishable, totalValue, isLowStock, isExpensive, isExpiring)
