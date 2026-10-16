use std::path::{Path, PathBuf};

/// Find a Taida project root by walking up from `start_dir`.
///
/// `.taida/` is state/config storage, not a project-root marker. Markers
/// (`packages.tdm`, `taida.toml`, `.git`) are ignored when they sit directly
/// in a shared temporary directory (see [`is_shared_temp_dir`]).
pub(crate) fn find_project_root(start_dir: &Path) -> PathBuf {
    let mut dir = start_dir.to_path_buf();
    loop {
        if (dir.join("packages.tdm").exists()
            || dir.join("taida.toml").exists()
            || has_project_git_marker(&dir))
            && !is_shared_temp_dir(&dir)
        {
            return dir;
        }
        if !dir.pop() {
            break;
        }
    }
    start_dir.to_path_buf()
}

/// Whether `dir` is a shared temporary directory: the system temp
/// directory itself, or (on Unix) any world-writable directory with the
/// sticky bit, such as `/tmp`, `/var/tmp` or `/dev/shm`.
///
/// Every local user can create entries in such a directory, so a
/// `packages.tdm`, `taida.toml`, `.git` or `.taida/deps/` found there says
/// nothing about the project of a script below it. It is never used as a
/// project root, and its `.taida/deps/` is never searched for packages.
pub fn is_shared_temp_dir(dir: &Path) -> bool {
    if paths_equal(dir, &std::env::temp_dir()) {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        const STICKY_AND_WORLD_WRITABLE: u32 = 0o1002;
        if let Ok(meta) = std::fs::metadata(dir) {
            return meta.permissions().mode() & STICKY_AND_WORLD_WRITABLE
                == STICKY_AND_WORLD_WRITABLE;
        }
    }
    false
}

fn has_project_git_marker(dir: &Path) -> bool {
    let git = dir.join(".git");
    if !git.exists() {
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
