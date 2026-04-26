// terminal_spinner.td -- ASCII spinner using taida-lang/terminal
//
// Usage (Interpreter, requires terminal addon installed):
//   taida install taida-lang/terminal
//   taida examples/terminal_spinner.td
//
// Renders 8 spinner frames in succession via SpinnerNext recursion.
// Each frame overwrites the previous one with ClearLine + Write.

>>> taida-lang/terminal => @(SpinnerState, SpinnerNext, SpinnerRender, Write, ClearLine)

renderTick state remaining: Int =
  | remaining < 1 |> 0
  | _ |>
    bytes <= Write[](ClearLine())
    rendered <= Write[](SpinnerRender(state))
    next <= SpinnerNext(state)
    renderTick(next, remaining - 1)
=> :Int

initial <= SpinnerState
total <= renderTick(initial, 8)
final_clear <= Write[](ClearLine())
stdout("done after " + (total + 8).toString() + " ticks")
