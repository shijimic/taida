// display.td -- Display helpers for inventory items
//
// Demonstrates: conditional branches, mold pipelines, field access on inherited types

>>> ./models.td => @(totalValue, isLowStock, isExpiring)

// C20-1 (ROOT-5): multi-line rhs `| |>` guards are now rejected with
// `[E0303]`. Wrap the branch in parens (the canonical escape hatch)
// so the reader can see the boundary at a glance and the parser
// cannot greedy-consume the next statement.
printItem item =
  lowTag <= (
    | isLowStock(item) |> " [LOW]"
    | _ |> ""
  )
  stdout("  " + item.name + " - $" + item.price.toString() + " x " + item.qty.toString() + " = $" + totalValue(item).toString() + lowTag)
  0
=> :Int

printPerishable item =
  tag <= (
    | isExpiring(item) |> " [EXPIRING]"
    | _ |> ""
  )
  stdout("  " + item.name + " - $" + item.price.toString() + " x " + item.qty.toString() + " (days left: " + item.daysLeft.toString() + ")" + tag)
  0
=> :Int

printItems label items =
  stdout(label)
  Map[items, printItem]() ]=> shown
  stdout("")
  0
=> :Int

<<< @(printItem, printPerishable, printItems)
