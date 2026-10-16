//! Console input on the compiled backends: `stdinLine` shows its prompt only
//! to a person at a terminal, as the interpreter's line editor does, so a
//! program reading from a file, a pipe or a closed input prints no prompt.
#![cfg(feature = "native")]
mod common;
mod f65_support;
use f65_support::parity;

#[test]
fn stdin_line_prompts_only_at_a_terminal() {
    parity(
        "f65_stdin_line_prompt",
        r#"stdinLine("Name: ") >=> line
stdout("Hello, " + line.getOrDefault("?"))
"#,
        "Hello, ?",
        &["native"],
    );
}

#[test]
fn create_dir_on_a_file_and_run_of_a_missing_program_are_io_errors() {
    // `createDir` succeeds on an existing directory only, and a program
    // that cannot be started is an IoError carrying the errno, as on the
    // interpreter.
    parity(
        "f65_os_failures",
        r#">>> taida-lang/os => @(writeFile, createDir, remove, run)
writeFile("plain.txt", "data") >=> w
r <= createDir("plain.txt")
stdout(r.isError())
createDir("made") >=> first
createDir("made") >=> again
stdout(first.toString() + " " + again.toString())
remove("made") >=> gone
g <= run("/nonexistent/program-xyz", @[])
g.errorInfo() >=> info
stdout(info.type + " " + info.kind + " " + info.code.toString())
"#,
        "true\n1 0\nIoError not_found 2",
        &["native"],
    );
}

#[test]
fn a_process_that_fails_gives_its_exit_code() {
    // A non-zero exit is a ProcessError whose `code` is the exit status; a
    // child ended by a signal has `128 + signal`.
    parity(
        "f65_process_exit_codes",
        r#">>> taida-lang/os => @(run, execShell)
g <= run("false", @[])
g.errorInfo() >=> info
stdout(info.type + " " + info.kind + " " + info.code.toString())
k <= execShell("kill -9 $$")
k.errorInfo() >=> killed
stdout(killed.type + " " + killed.code.toString())
"#,
        "ProcessError ProcessError 1\nProcessError 137",
        &["native"],
    );
}
