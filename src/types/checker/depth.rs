//! Checking a program whose expressions nest deeply.
//!
//! The checker walks an expression by recursion, one level of the tree per
//! call. A left-associative chain such as `1 + 1 + ... + 1` is a tree as
//! deep as the chain is long, and a few thousand terms use up the default
//! thread stack. The check of such a program runs on a thread with a stack
//! sized for its depth; a program too deep even for that is reported
//! instead of overflowing the stack of whatever called the checker.

use super::{TypeChecker, TypeError};
use crate::graph::walk;
use crate::lexer::Span;
use crate::parser::*;

impl TypeChecker {
    /// Check an entire program. Collects type definitions first,
    /// then checks all statements.
    pub fn check_program(&mut self, program: &Program) {
        let (depth, deepest) = walk::nesting_depth(program);
        if depth > walk::MAX_DEPTH {
            self.errors.push(TypeError {
                message: format!(
                    "Expression nested too deeply to type-check ({depth} levels, the limit is {}). Split it into several statements.",
                    walk::MAX_DEPTH
                ),
                span: deepest.unwrap_or_else(|| Span::new(0, 0, 1, 1)),
            });
            return;
        }
        walk::with_stack_for_depth(depth, || self.check_program_body(program));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(terms: usize) -> String {
        format!("x <= {}\n", vec!["1"; terms].join(" + "))
    }

    fn check(source: String) -> Vec<String> {
        // Parsing numbers the nodes of the tree, which recurses too, so it is
        // done on a large stack: a test thread's is small.
        let program = std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn(move || {
                let (program, errors) = crate::parser::parse(&source);
                assert!(errors.is_empty(), "parse errors: {errors:?}");
                program
            })
            .expect("spawn a parsing thread")
            .join()
            .expect("parse the chain");
        let mut checker = TypeChecker::new();
        checker.check_program(&program);
        let messages = checker.errors.iter().map(|e| e.message.clone()).collect();
        // Dropping a tree this deep recurses too, and the stack of a test
        // thread is small: the tree is left to the end of the process.
        std::mem::forget(program);
        messages
    }

    #[test]
    fn a_long_left_associative_chain_is_checked_without_overflowing() {
        // Far past what the 2 MiB stack of a test thread can recurse over.
        assert_eq!(check(chain(6000)), Vec::<String>::new());
    }

    #[test]
    fn a_program_deeper_than_the_limit_is_reported() {
        let messages = check(chain(walk::MAX_DEPTH + 10));
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(messages[0].contains("nested too deeply"), "{}", messages[0]);
    }
}
