use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::parser::{Statement, parse};

#[derive(Debug, Clone)]
pub enum ModuleGraphError {
    Io { path: PathBuf, message: String },
    Parse { path: PathBuf, message: String },
    Circular { path: PathBuf },
}

impl std::fmt::Display for ModuleGraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleGraphError::Io { path, message } => {
                write!(f, "Cannot read module '{}': {}", path.display(), message)
            }
            ModuleGraphError::Parse { path, message } => {
                write!(
                    f,
                    "Parse errors in module '{}': {}",
                    path.display(),
                    message
                )
            }
            ModuleGraphError::Circular { path } => {
                write!(f, "Circular import detected: '{}'", path.display())
            }
        }
    }
}

pub fn detect_local_import_cycle(entry_path: &Path) -> Result<(), ModuleGraphError> {
    collect_local_modules(entry_path).map(|_| ())
}

pub fn collect_local_modules(entry_path: &Path) -> Result<Vec<PathBuf>, ModuleGraphError> {
    let entry = canonicalize_or_original(entry_path);
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    let mut ordered = Vec::new();
    visit_module(&entry, &mut visiting, &mut visited, &mut ordered)?;
    Ok(ordered)
}

/// Resolve a local import path to an absolute path.
/// Returns `None` for package imports (e.g., `author/pkg@ver`) since those point to
/// `.taida/deps/` and cannot form circular dependencies with local modules.
/// Package dependency cycles are handled at the package manager level, not here.
pub fn resolve_local_import_from(base_file: &Path, import_path: &str) -> Option<PathBuf> {
    let path = if import_path.starts_with("./") || import_path.starts_with("../") {
        base_file
            .parent()
            .unwrap_or(Path::new("."))
            .join(import_path)
    } else if let Some(stripped) = import_path.strip_prefix("~/") {
        let home = crate::util::taida_home_dir().ok()?;
        home.join(stripped)
    } else if import_path.starts_with('/') {
        PathBuf::from(import_path)
    } else {
        // Package imports (e.g., `author/pkg`, `author/pkg@ver`) are external
        // dependencies and not part of the local module graph.
        return None;
    };

    Some(path)
}

fn visit_module(
    path: &Path,
    visiting: &mut HashSet<PathBuf>,
    visited: &mut HashSet<PathBuf>,
    ordered: &mut Vec<PathBuf>,
) -> Result<(), ModuleGraphError> {
    let canonical = canonicalize_or_original(path);
    if visiting.contains(&canonical) {
        return Err(ModuleGraphError::Circular { path: canonical });
    }
    if visited.contains(&canonical) {
        return Ok(());
    }

    let source = fs::read_to_string(&canonical).map_err(|e| ModuleGraphError::Io {
        path: canonical.clone(),
        message: e.to_string(),
    })?;
    let (program, parse_errors) = parse(&source);
    if !parse_errors.is_empty() {
        let message = parse_errors
            .iter()
            .map(|err| err.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(ModuleGraphError::Parse {
            path: canonical,
            message,
        });
    }

    visiting.insert(canonical.clone());
    for stmt in &program.statements {
        if let Statement::Import(import) = stmt
            && let Some(dep_path) = resolve_local_import_from(&canonical, &import.path)
        {
            visit_module(&dep_path, visiting, visited, ordered)?;
        }
    }
    visiting.remove(&canonical);
    visited.insert(canonical.clone());
    ordered.push(canonical);
    Ok(())
}

fn canonicalize_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
