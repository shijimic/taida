use std::path::PathBuf;

/// Resolve the user's home directory.
/// Checks HOME first, then USERPROFILE (Windows fallback).
/// Returns an error if neither is set.
pub fn taida_home_dir() -> Result<PathBuf, String> {
    std::env::var("HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("USERPROFILE").ok().filter(|v| !v.is_empty()))
        .map(PathBuf::from)
        .ok_or_else(|| "Home directory not found: neither HOME nor USERPROFILE is set".to_string())
}

/// Shared lock for tests that modify environment variables.
///
/// All tests across modules that call `std::env::set_var` / `std::env::remove_var`
/// must acquire this lock to prevent data races.
#[cfg(test)]
pub fn env_test_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// Acquire the env lock, recovering from poisoning. A single test
/// panicking while it holds the lock must not cascade a PoisonError
/// into every later env-touching test in the same process — each test
/// sets up the variables it reads, so the previous holder's state is
/// irrelevant to correctness.
#[cfg(test)]
pub fn env_test_guard() -> std::sync::MutexGuard<'static, ()> {
    env_test_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Compare generations in their published order: a..z, aa..zz, aaa...
pub(crate) fn compare_generation(a: &str, b: &str) -> std::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// Prefer an unlabelled release, then the stable label, then lexical labels.
pub(crate) fn compare_version_labels(a: Option<&str>, b: Option<&str>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => (a == "stable").cmp(&(b == "stable")).then_with(|| a.cmp(b)),
    }
}

/// CLI HTTP requests have finite connection and response deadlines.
#[cfg(feature = "community")]
pub(crate) fn http_client_builder() -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(120))
}

#[cfg(feature = "community")]
pub(crate) fn http_client() -> Result<reqwest::blocking::Client, String> {
    http_client_builder()
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}
