// RC1.5-4c: minimal terminal addon e2e example
// Uses all 5 functions: termPrint, termPrintLn, termReadLine, termSize, termIsTty

import "taida-lang/terminal" => term

term "hello" |> term.termPrint
"done" |> term.termPrintLn

@(rows, cols) <= term.termSize
rows => print

term.termIsTty => print
