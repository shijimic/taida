// todo_app.td -- TODO List Manager Demo
//
// Demonstrates: List operations, conditions, error handling,
// and mold-based list operations (Find, Count, Sort, Take, Drop, Zip, Enumerate)
//
// Note: Taida variables are immutable. Each operation returns a new list.


// -- Helper Functions --

doneLabel b =
  | b |> "[x]"
  | _ |> "[ ]"
=> :Str

showOneTodo pair =
  idx <= pair.index + 1
  todo <= pair.value
  status <= doneLabel(todo.done)
  stdout("  " + idx.toString() + ". " + status + " " + todo.title)
  0
=> :Int

showTodos list =
  enumerated <= Enumerate[list]()
  Map[enumerated, showOneTodo]() ]=> shown
  0
=> :Int

addTodo list title =
  new_item <= @(title <= title, done <= false)
  Append[list, new_item]()

toggleAt index pair =
  | pair.index == index |>
    @(title <= pair.value.title, done <= !pair.value.done)
  | _ |> pair.value

keepOther index pair =
  pair.index != index
=> :Bool

extractValue pair =
  pair.value

isDone x =
  x.done
=> :Bool

isNotDone x =
  !x.done
=> :Bool

// -- Main --

stdout("=== Taida TODO Manager ===")
stdout("")

// Build the list step by step (immutable: each step yields a new list)
list1 <= addTodo(@[], "Learn Taida basics")
list2 <= addTodo(list1, "Build a TODO app")
list3 <= addTodo(list2, "Write tests")

stdout("--- After adding 3 items ---")
showTodos(list3)

// Complete items 0 and 1
enum3 <= Enumerate[list3]()
Map[enum3, _ pair = toggleAt(0, pair)]() ]=> list4
enum4 <= Enumerate[list4]()
Map[enum4, _ pair = toggleAt(1, pair)]() ]=> list5

stdout("")
stdout("--- After completing 2 items ---")
showTodos(list5)

// Statistics using mold operations
total <= list5.length()
done_count <= Count[list5, isDone]()
pending_count <= total - done_count
stdout("")
stdout("--- Statistics ---")
stdout("  Total: " + total.toString() + " | Done: " + done_count.toString() + " | Pending: " + pending_count.toString())

allDone <= list5.all(isDone)
| allDone |> stdout("  All tasks complete!")
| _ |> stdout("  Some tasks remain.")

// Find[] -- locate first pending item
pending_todo <= Find[list5, isNotDone]()
stdout("")
| pending_todo.hasValue |>
  stdout("Next pending: " + pending_todo.__value.title)
| _ |>
  stdout("No pending tasks!")

// .any() -- quick check
hasAnyDone <= list5.any(isDone)
stdout("")
stdout("Has any completed: " + hasAnyDone.toString())

// Remove item at index 1
enum5 <= Enumerate[list5]()
Filter[enum5, _ pair = keepOther(1, pair)]() ]=> filtered
Map[filtered, extractValue]() ]=> list6

stdout("")
stdout("--- After removing item 2 ---")
showTodos(list6)

// Take[] / Drop[]
stdout("")
stdout("--- First 1 item (take) ---")
first_one <= Take[list6, 1]()
showTodos(first_one)

stdout("")
stdout("--- Remaining after drop 1 ---")
remaining <= Drop[list6, 1]()
showTodos(remaining)

// Sort[] with by option (descending) demo with numbers
neg x =
  0 - x
=> :Int

nums <= @[5, 2, 8, 1, 9]
sorted_desc <= Sort[nums](by <= neg)
stdout("")
stdout("--- sortBy (descending) ---")
stdout("  " + Join[sorted_desc, ", "]())

// Zip[] demo
pilot_names <= @["Asuka", "Rei", "Shinji"]
pilot_scores <= @[95, 87, 92]
zipped <= Zip[pilot_names, pilot_scores]()

showPair p =
  stdout("  " + p.first + ": " + p.second.toString())
  0
=> :Int

stdout("")
stdout("--- Zip example ---")
Map[zipped, showPair]() ]=> zipResult

// FindIndex[] demo
idx <= FindIndex[@[10, 20, 30, 40, 50], _ x = x > 25]()
stdout("")
stdout("--- findIndex (first > 25) ---")
stdout("  Index: " + idx.toString())

stdout("")
stdout("=== Done ===")
