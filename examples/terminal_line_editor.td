// terminal_line_editor.td -- Line editor demo using taida-lang/terminal
//
// Usage (Interpreter, requires terminal addon installed):
//   taida install taida-lang/terminal
//   taida examples/terminal_line_editor.td
//
// Initialises the line editor state with a prompt and an initial value,
// then renders one frame so the editor surface is exercised. Real
// line editing requires raw mode + ReadEvent dispatching; this example
// keeps to the pure portion of the API to stay friendly to non-TTY
// environments (CI, redirected stdin).

>>> taida-lang/terminal => @(LineEditorNew, LineEditorRender, PromptOptions, PromptMode, Write)

opts <= PromptOptions(
  prompt <= "name> ",
  initial <= "Misato",
  placeholder <= "",
  mode <= PromptMode.Normal,
  history <= @[],
  completion <= @()
)

editor <= LineEditorNew(opts)
view <= LineEditorRender(editor)
bytes <= Write[](view.text)
stdout("\nline editor frame written: " + bytes.toString() + " bytes")
stdout("editor cursor=" + editor.cursor.toString() + " text=" + editor.text)
