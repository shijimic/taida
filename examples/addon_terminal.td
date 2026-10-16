// Terminal addon example.
//
// Calls the workspace sample addon `crates/addon-terminal-sample`, which
// is published as package `taida-lang/terminal` with five functions:
// termPrint, termPrintLn, termReadLine, termSize, termIsTty. (The official
// `taida-lang/terminal` package has a larger, different surface.)
//
// To run it, build the sample (`cargo build -p taida-addon-terminal-sample`)
// and place its cdylib and `native/addon.toml` under
// `.taida/deps/taida-lang/terminal/`; `tests/addon_terminal_install_e2e.rs`
// sets up the same layout.

>>> taida-lang/terminal => @(termPrint, termPrintLn, termSize, termIsTty)

// termSize() needs a terminal, so ask termIsTty() first.
describeSize tty: Bool =
  | tty |> `${termSize().cols} columns x ${termSize().rows} rows`
  | _ |> "not a terminal"
=> :Str

termPrint("hello ")
termPrintLn("from the terminal addon")

tty: Bool <= termIsTty()
stdout(tty)
stdout(describeSize(tty))
