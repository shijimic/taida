// operations.td -- Inventory operations (search, filter, aggregation)
//
// Demonstrates: molds (Filter, Map, Find, Sum), Lax, throw/|==

>>> ./models.td => @(StockedItem, PerishableItem, StockError, totalValue, isLowStock, isExpensive, isExpiring)

findByName items: @[StockedItem] name: Str =
  Find[items, _ item: StockedItem = item.name == name]()
=> :Lax[StockedItem]

calcTotalValue items: @[StockedItem] =
  Map[items, totalValue]() >=> values
  Sum[values]()
=> :Int

lowStockReport items: @[StockedItem] =
  Filter[items, isLowStock]() >=> result
  result
=> :@[StockedItem]

expensiveReport items: @[StockedItem] =
  Filter[items, isExpensive]() >=> result
  result
=> :@[StockedItem]

expiringReport items: @[PerishableItem] =
  Filter[items, isExpiring]() >=> result
  result
=> :@[PerishableItem]

// Validate stock level — throws StockError if qty is 0, catches internally
validateStock item: StockedItem =
  |== err: StockError =
    "Out of stock"
  => :Str
  | item.qty == 0 |>
    err <= StockError(message <= "Out of stock", reason <= "zero quantity", item <= item.name)
    err.throw()
  | _ |>
    "stock OK"
=> :Str

<<< @(findByName, calcTotalValue, lowStockReport, expensiveReport, expiringReport, validateStock)
