use std::path::{Path, PathBuf};

/// Find a Taida project root by walking up from `start_dir`.
///
/// `.taida/` is state/config storage, not a project-root marker. A `.git`
/// directory is accepted for normal projects, but not when it is the ambient
/// marker on the system temp directory itself.
pub(crate) fn find_project_root(start_dir: &Path) -> PathBuf {
    let mut dir = start_dir.to_path_buf();
    loop {
        if dir.join("packages.tdm").exists()
            || dir.join("taida.toml").exists()
            || has_project_git_marker(&dir)
        {
            return dir;
        }
        if !dir.pop() {
            break;
        }
    }
    start_dir.to_path_buf()
}

fn has_project_git_marker(dir: &Path) -> bool {
    let git = dir.join(".git");
    if !git.exists() {
        return false;
    }

    let temp_dir = std::env::temp_dir();
    if paths_equal(dir, &temp_dir) {
        return false;
    }

    if git.is_file() {
        return std::fs::read_to_string(&git)
            .map(|contents| contents.trim_start().starts_with("gitdir:"))
            .unwrap_or(false);
    }

    git.join("HEAD").is_file() || git.join("objects").is_dir()
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

#[cfg(test)]
mod tests {
    use super::find_project_root;
    use std::path::{Path, PathBuf};

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "taida_project_root_{}_{}_{}",
                name,
                std::process::id(),
                nanos
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create project root test dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn accepts_valid_git_dir_marker() {
        let root = TestDir::new("valid_git");
        let nested = root.path().join("src").join("deep");
        std::fs::create_dir_all(root.path().join(".git").join("objects"))
            .expect("create git objects");
        std::fs::write(
            root.path().join(".git").join("HEAD"),
            "ref: refs/heads/main\n",
        )
        .expect("write git HEAD");
        std::fs::create_dir_all(&nested).expect("create nested dir");

        assert_eq!(find_project_root(&nested), root.path());
    }

    #[test]
    fn ignores_empty_dot_git_directory() {
        let root = TestDir::new("empty_git");
        let nested = root.path().join("src").join("deep");
        std::fs::create_dir_all(root.path().join(".git")).expect("create empty .git");
        std::fs::create_dir_all(&nested).expect("create nested dir");

        assert_eq!(find_project_root(&nested), nested);
    }
}
