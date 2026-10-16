/// Environment (scope) for the Taida interpreter.
///
/// Names resolve lexically (`docs/reference/scope_rules.md`):
/// - Module scope: top-level definitions of the module whose top-level
///   code is running (`globals`).
/// - Frames: function parameters and locals, the scopes a closure
///   captured, and block scopes (condition arms, error-ceiling handlers,
///   pipeline bindings).
///
/// Every frame links to its lexical parent. A function call pushes its
/// frames on top of the caller's, but links them to the scopes that
/// enclosed the function's *definition* — so a callee never sees its
/// caller's locals, while the caller's frames stay in place underneath.
/// A chain ends either at `globals` or at a frame that holds a finished
/// module's top-level scope.
///
/// - No shadowing within the same scope
/// - All variables are immutable
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::value::Value;

/// Frame ids are unique across every `Environment` of the process, so a
/// function value that remembers the frame it was defined in (see
/// `definition_site`) can never mistake an unrelated frame — one pushed
/// by another module's environment, or pushed later at the same stack
/// index — for its own.
static NEXT_FRAME_ID: AtomicU64 = AtomicU64::new(1);

/// Where a frame's lexical chain continues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// Another frame on the stack, by index.
    Frame(usize),
    /// The top-level scope of the module whose top-level code is running.
    Globals,
    /// The chain ends here.
    End,
}

#[derive(Debug, Clone)]
enum Vars {
    Owned(HashMap<String, Value>),
    /// A read-only scope shared with a function value (a closure
    /// capture) or with a finished module (its top-level scope).
    Shared(Arc<HashMap<String, Value>>),
}

impl Vars {
    fn get(&self, name: &str) -> Option<&Value> {
        match self {
            Vars::Owned(map) => map.get(name),
            Vars::Shared(map) => map.get(name),
        }
    }

    fn contains_key(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    fn owned_mut(&mut self) -> &mut HashMap<String, Value> {
        if let Vars::Shared(map) = self {
            *self = Vars::Owned(map.as_ref().clone());
        }
        match self {
            Vars::Owned(map) => map,
            Vars::Shared(_) => unreachable!("converted to an owned scope above"),
        }
    }

    fn iter(&self) -> std::collections::hash_map::Iter<'_, String, Value> {
        match self {
            Vars::Owned(map) => map.iter(),
            Vars::Shared(map) => map.iter(),
        }
    }
}

#[derive(Debug, Clone)]
struct Frame {
    vars: Vars,
    parent: Link,
    id: u64,
    /// The frame stands in for a finished module's top-level scope. Such
    /// names are resolved when a function runs, so closures never copy
    /// them.
    module_scope: bool,
}

/// The interpreter's variable environment.
#[derive(Debug, Clone)]
pub struct Environment {
    /// Top-level scope of the module whose top-level code is running.
    globals: HashMap<String, Value>,
    /// Module-scope names that came from an import without a symbol list
    /// (`>>> ./utils.td`). The module's own definitions take precedence
    /// over them.
    star_imported: std::collections::HashSet<String>,
    /// Frames, innermost last. Parent links always point to a lower index.
    frames: Vec<Frame>,
}

impl Environment {
    /// Create a new environment with an empty module scope.
    pub fn new() -> Self {
        Self {
            globals: HashMap::new(),
            star_imported: std::collections::HashSet::new(),
            frames: Vec::new(),
        }
    }

    fn top_link(&self) -> Link {
        match self.frames.len() {
            0 => Link::Globals,
            n => Link::Frame(n - 1),
        }
    }

    fn push_frame(&mut self, vars: Vars, parent: Link, module_scope: bool) -> Link {
        let id = NEXT_FRAME_ID.fetch_add(1, Ordering::Relaxed);
        self.frames.push(Frame {
            vars,
            parent,
            id,
            module_scope,
        });
        Link::Frame(self.frames.len() - 1)
    }

    /// Push a block scope (condition arm, handler, pipeline binding, ...)
    /// nested in the innermost scope.
    pub fn push_scope(&mut self) {
        let parent = self.top_link();
        self.push_frame(Vars::Owned(HashMap::new()), parent, false);
    }

    /// Pop the innermost frame. The module scope is never popped.
    pub fn pop_scope(&mut self) {
        self.frames.pop();
    }

    /// Current height of the frame stack, for `truncate`.
    pub fn mark(&self) -> usize {
        self.frames.len()
    }

    /// Drop every frame pushed since `mark` was taken. Cleanup paths use
    /// this instead of counting pops, so a frame left behind by an inner
    /// early return can never outlive the construct that pushed it.
    pub fn truncate(&mut self, mark: usize) {
        self.frames.truncate(mark);
    }

    /// Push a finished module's top-level scope as the end of a chain.
    pub fn push_module_scope(&mut self, scope: Arc<HashMap<String, Value>>) -> Link {
        self.push_frame(Vars::Shared(scope), Link::End, true)
    }

    /// Push a read-only scope (a closure capture) linked to `parent`.
    pub fn push_shared_scope(&mut self, vars: Arc<HashMap<String, Value>>, parent: Link) -> Link {
        self.push_frame(Vars::Shared(vars), parent, false)
    }

    /// Push a fresh scope linked to `parent` (a function's local scope).
    pub fn push_linked_scope(&mut self, vars: HashMap<String, Value>, parent: Link) -> Link {
        self.push_frame(Vars::Owned(vars), parent, false)
    }

    /// The innermost frame as a definition site: `(index, id)`. `None` at
    /// module top level.
    pub fn definition_site(&self) -> Option<(usize, u64)> {
        self.frames.last().map(|f| (self.frames.len() - 1, f.id))
    }

    /// Link to the frame recorded by `definition_site`, if that frame is
    /// still on the stack.
    pub fn live_site(&self, site: (usize, u64)) -> Option<Link> {
        let (index, id) = site;
        self.frames
            .get(index)
            .filter(|frame| frame.id == id)
            .map(|_| Link::Frame(index))
    }

    fn current_scope_mut(&mut self) -> &mut HashMap<String, Value> {
        match self.frames.last_mut() {
            Some(frame) => frame.vars.owned_mut(),
            None => &mut self.globals,
        }
    }

    /// Define a variable in the current (innermost) scope.
    /// Returns an error if the variable is already defined in the same scope.
    pub fn define(&mut self, name: &str, value: Value) -> Result<(), String> {
        if self.frames.is_empty() && self.star_imported.remove(name) {
            // A definition of the module itself replaces a name an import
            // without a symbol list brought in.
            self.globals.insert(name.to_string(), value);
            return Ok(());
        }
        let scope = self.current_scope_mut();
        if scope.contains_key(name) {
            return Err(format!(
                "Variable '{}' is already defined in this scope",
                name
            ));
        }
        scope.insert(name.to_string(), value);
        Ok(())
    }

    /// Bind a name brought in by an import without a symbol list. A name
    /// the scope already binds keeps its binding.
    pub fn define_star_import(&mut self, name: &str, value: Value) {
        if self.frames.is_empty() {
            if !self.globals.contains_key(name) {
                self.globals.insert(name.to_string(), value);
                self.star_imported.insert(name.to_string());
            }
        } else if !self.is_defined_in_current_scope(name) {
            self.define_force(name, value);
        }
    }

    /// Define or overwrite a variable in the current scope.
    /// Used for built-in definitions that may be redefined.
    pub fn define_force(&mut self, name: &str, value: Value) {
        self.current_scope_mut().insert(name.to_string(), value);
    }

    /// Look up a variable along the lexical chain of the innermost scope.
    pub fn get(&self, name: &str) -> Option<&Value> {
        let mut link = self.top_link();
        loop {
            match link {
                Link::Frame(index) => {
                    let frame = &self.frames[index];
                    if let Some(value) = frame.vars.get(name) {
                        return Some(value);
                    }
                    link = frame.parent;
                }
                Link::Globals => return self.globals.get(name),
                Link::End => return None,
            }
        }
    }

    /// Check if a variable is visible from the innermost scope.
    pub fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Remove and return a binding, resolving the name exactly as `get`
    /// does, so shadowing resolves identically. The caller takes
    /// ownership of the value; the binding ceases to exist. Only safe
    /// when the caller can prove the name is never read again in the
    /// binding's remaining lifetime (see the Append consume fast path).
    /// A binding held by a shared (read-only) scope is never taken.
    pub fn take(&mut self, name: &str) -> Option<Value> {
        let mut link = self.top_link();
        loop {
            match link {
                Link::Frame(index) => {
                    let parent = self.frames[index].parent;
                    match &mut self.frames[index].vars {
                        Vars::Owned(map) => {
                            if let Some(value) = map.remove(name) {
                                return Some(value);
                            }
                        }
                        Vars::Shared(map) => {
                            if map.contains_key(name) {
                                return None;
                            }
                        }
                    }
                    link = parent;
                }
                Link::Globals => return self.globals.remove(name),
                Link::End => return None,
            }
        }
    }

    /// Check if a variable is defined in the current (innermost) scope only.
    /// Used to distinguish local definitions from outer-scope definitions.
    pub fn is_defined_in_current_scope(&self, name: &str) -> bool {
        match self.frames.last() {
            Some(frame) => frame.vars.contains_key(name),
            None => self.globals.contains_key(name),
        }
    }

    /// Capture the scopes a closure defined here can see: the lexical
    /// chain of the innermost scope, excluding module scopes. Top-level
    /// names are not captured — a function resolves them in its module
    /// when it runs, which is what lets top-level functions refer to
    /// each other in any definition order.
    pub fn snapshot(&self) -> HashMap<String, Value> {
        let mut result = HashMap::new();
        let mut link = self.top_link();
        while let Link::Frame(index) = link {
            let frame = &self.frames[index];
            if !frame.module_scope {
                for (k, v) in frame.vars.iter() {
                    if !result.contains_key(k) {
                        result.insert(k.clone(), v.clone());
                    }
                }
            }
            link = frame.parent;
        }
        result
    }

    /// The module scope (top-level bindings of the running module).
    pub fn globals(&self) -> &HashMap<String, Value> {
        &self.globals
    }

    /// Consume the environment, returning its module scope.
    pub fn into_globals(self) -> HashMap<String, Value> {
        self.globals
    }

    /// Get the current scope depth (the module scope counts as one).
    pub fn depth(&self) -> usize {
        self.frames.len() + 1
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
    fn test_snapshot_captures_frames_not_module_scope() {
        let mut env = Environment::new();
        env.define("x", Value::Int(1)).unwrap();
        env.push_scope();
        env.define("y", Value::Int(2)).unwrap();

        let snap = env.snapshot();
        assert_eq!(snap.get("x"), None);
        assert_eq!(snap.get("y"), Some(&Value::Int(2)));
    }

    #[test]
    fn test_linked_scope_hides_unrelated_frames() {
        let mut env = Environment::new();
        env.define("g", Value::Int(1)).unwrap();
        // A caller's local scope.
        env.push_scope();
        env.define("callerLocal", Value::Int(2)).unwrap();
        // A callee linked straight to the module scope.
        let mark = env.mark();
        env.push_linked_scope(HashMap::new(), Link::Globals);
        assert_eq!(env.get("g"), Some(&Value::Int(1)));
        assert_eq!(env.get("callerLocal"), None);
        env.truncate(mark);
        assert_eq!(env.get("callerLocal"), Some(&Value::Int(2)));
    }

    #[test]
    fn test_module_scope_ends_the_chain() {
        let mut env = Environment::new();
        env.define("mine", Value::Int(1)).unwrap();
        let mut other = HashMap::new();
        other.insert("theirs".to_string(), Value::Int(2));
        let link = env.push_module_scope(Arc::new(other));
        env.push_linked_scope(HashMap::new(), link);
        assert_eq!(env.get("theirs"), Some(&Value::Int(2)));
        assert_eq!(env.get("mine"), None);
        assert!(env.snapshot().is_empty());
    }

    #[test]
    fn test_definition_site_tracks_frame_liveness() {
        let mut env = Environment::new();
        env.push_scope();
        let site = env.definition_site().expect("inside a frame");
        assert_eq!(env.live_site(site), Some(Link::Frame(0)));
        env.pop_scope();
        assert_eq!(env.live_site(site), None);
        // A new frame at the same index is a different frame.
        env.push_scope();
        assert_eq!(env.live_site(site), None);
    }
}
