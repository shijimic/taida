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

>>> taida-lang/terminal => @(lineEditorNew, lineEditorRender, PromptOptions, PromptMode, write)

opts <= PromptOptions(
  prompt <= "name> ",
  initial <= "Misato",
  placeholder <= "",
  mode <= PromptMode.normal,
  history <= @[],
  completion <= @()
)

editor <= lineEditorNew(opts)
view <= lineEditorRender(editor)
bytes <= write(view.line)
stdout("\nline editor frame written: " + bytes.toString() + " bytes")
stdout("editor cursor=" + editor.cursor.toString() + " text=" + editor.text)
