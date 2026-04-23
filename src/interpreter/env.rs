/// Environment (scope) for the Taida interpreter.
///
/// Scope rules:
/// - Module scope: top-level definitions
/// - Function scope: function parameters and local variables
/// - Lexical scoping with closures
/// - No shadowing within same scope
/// - All variables are immutable
use std::collections::HashMap;

use super::value::Value;

/// An environment frame — one scope level.
#[derive(Debug, Clone)]
pub struct Environment {
    /// Stack of scopes (last = innermost)
    scopes: Vec<HashMap<String, Value>>,
}

impl Environment {
    /// Create a new environment with a global (module) scope.
    pub fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
        }
    }

    /// Push a new scope (entering a function or block).
    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Pop the innermost scope (leaving a function or block).
    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// Define a variable in the current (innermost) scope.
    /// Returns an error if the variable is already defined in the same scope.
    ///
    /// # Invariant
    /// `scopes` is never empty: `new()` initializes with one scope, and
    /// `pop_scope()` guards against popping the last element.
    pub fn define(&mut self, name: &str, value: Value) -> Result<(), String> {
        // SAFETY: scopes is always non-empty (invariant enforced by new/pop_scope)
        let scope = self
            .scopes
            .last_mut()
            .expect("scope stack must be non-empty");
        if scope.contains_key(name) {
            return Err(format!(
                "Variable '{}' is already defined in this scope",
                name
            ));
        }
        scope.insert(name.to_string(), value);
        Ok(())
    }

    /// Define or overwrite a variable in the current scope.
    /// Used for built-in definitions that may be redefined.
    ///
    /// # Invariant
    /// See `define()` — scopes is always non-empty.
    pub fn define_force(&mut self, name: &str, value: Value) {
        // SAFETY: scopes is always non-empty (invariant enforced by new/pop_scope)
        let scope = self
            .scopes
            .last_mut()
            .expect("scope stack must be non-empty");
        scope.insert(name.to_string(), value);
    }

    /// Look up a variable by name, searching from innermost to outermost scope.
    pub fn get(&self, name: &str) -> Option<&Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return Some(value);
            }
        }
        None
    }

    /// Check if a variable exists in any scope.
    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Check if a variable is defined in the current (innermost) scope only.
    /// Used to distinguish local definitions from outer-scope definitions.
    pub fn is_defined_in_current_scope(&self, name: &str) -> bool {
        self.scopes
            .last()
            .is_some_and(|scope| scope.contains_key(name))
    }

    /// Create a snapshot of the current environment for closures.
    pub fn snapshot(&self) -> HashMap<String, Value> {
        let mut result = HashMap::new();
        for scope in &self.scopes {
            for (k, v) in scope {
                result.insert(k.clone(), v.clone());
            }
        }
        result
    }

    /// Get the current scope depth.
    pub fn depth(&self) -> usize {
        self.scopes.len()
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_define_and_get() {
        let mut env = Environment::new();
        env.define("x", Value::Int(42)).unwrap();
        assert_eq!(env.get("x"), Some(&Value::Int(42)));
        assert_eq!(env.get("y"), None);
    }

    #[test]
    fn test_nested_scopes() {
        let mut env = Environment::new();
        env.define("x", Value::Int(1)).unwrap();

        env.push_scope();
        env.define("y", Value::Int(2)).unwrap();

        // Both visible in inner scope
        assert_eq!(env.get("x"), Some(&Value::Int(1)));
        assert_eq!(env.get("y"), Some(&Value::Int(2)));

        env.pop_scope();

        // Only outer variable visible
        assert_eq!(env.get("x"), Some(&Value::Int(1)));
        assert_eq!(env.get("y"), None);
    }

    #[test]
    fn test_shadowing_across_scopes() {
        let mut env = Environment::new();
        env.define("x", Value::Int(1)).unwrap();

        env.push_scope();
        // Shadowing in a different scope is allowed
        env.define("x", Value::Int(2)).unwrap();
        assert_eq!(env.get("x"), Some(&Value::Int(2)));

        env.pop_scope();
        assert_eq!(env.get("x"), Some(&Value::Int(1)));
    }

    #[test]
    fn test_no_redefinition_same_scope() {
        let mut env = Environment::new();
        env.define("x", Value::Int(1)).unwrap();
        let result = env.define("x", Value::Int(2));
        assert!(result.is_err());
    }

    #[test]
    fn test_snapshot() {
        let mut env = Environment::new();
        env.define("x", Value::Int(1)).unwrap();
        env.push_scope();
        env.define("y", Value::Int(2)).unwrap();

        let snap = env.snapshot();
        assert_eq!(snap.get("x"), Some(&Value::Int(1)));
        assert_eq!(snap.get("y"), Some(&Value::Int(2)));
    }
}
