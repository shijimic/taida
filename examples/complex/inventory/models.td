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
mkStocked name: Str price: Int qty: Int =
  StockedItem(name <= name, price <= price, qty <= qty)
=> :StockedItem

mkPerishable name: Str price: Int qty: Int days_left: Int =
  PerishableItem(name <= name, price <= price, qty <= qty, days_left <= days_left)
=> :PerishableItem

// --- Domain predicates ---
totalValue item: StockedItem =
  item.price * item.qty
=> :Int

isLowStock item: StockedItem =
  item.qty < 5
=> :Bool

isExpensive item: StockedItem =
  item.price > 100
=> :Bool

isExpiring item: PerishableItem =
  item.days_left < 3
=> :Bool

<<< @(Product, StockedItem, PerishableItem, InventoryError, StockError, mkStocked, mkPerishable, totalValue, isLowStock, isExpensive, isExpiring)
