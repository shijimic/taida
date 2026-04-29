#![allow(clippy::doc_overindented_list_items)]

//! C17-5: shared mock HTTP server for installer integration tests.
//!
//! Serves two endpoints that `taida ingot install` talks to:
//!
//! - `GET /<org>/<name>/archive/refs/tags/<version>.tar.gz`
//!   -> returns the currently-registered tarball bytes.
//! - `GET /repos/<org>/<name>/git/refs/tags/<version>`
//!   -> returns `{"ref":"refs/tags/<version>","object":{"sha":"<SHA>","type":"commit"}}`
//! - `GET /repos/<org>/<name>/tags?per_page=100`
//!   -> returns `[{"name":"<version>"}]` (single-entry tag list for
//!      generation resolution).
//!
//! Tests override `TAIDA_GITHUB_BASE_URL` and `TAIDA_GITHUB_API_URL` to
//! point at the server's `http://127.0.0.1:<port>` base. A shared
//! `TagState` (wrapped in `Arc<Mutex<...>>`) lets a test mutate the
//! tarball + SHA between calls to simulate a retag.
//!
//! The server uses blocking I/O in a single thread with a short read
//! timeout so a dropped `MockServer` cleans up deterministically.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Mutable state that tests swap between calls.
#[derive(Debug, Clone)]
pub struct TagState {
    pub org: String,
    pub name: String,
    pub version: String,
    /// 40-hex commit SHA returned for the tag's `git/refs/tags/<version>`.
    pub commit_sha: String,
    /// Raw `.tar.gz` bytes served for
    /// `/<org>/<name>/archive/refs/tags/<version>.tar.gz`.
    pub tarball: Vec<u8>,
}

pub struct MockServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockServer {
    pub fn start(state: Arc<Mutex<TagState>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        listener.set_nonblocking(false).expect("listener blocking");
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();

        let handle = std::thread::spawn(move || {
            loop {
                if stop_clone.load(Ordering::SeqCst) {
                    return;
                }
                // Check every accept for the stop flag. We use blocking accept
                // but a short inactivity nudge: on accept we set a read
                // timeout, handle, then loop.
                let (mut stream, _peer) = match listener.accept() {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                if stop_clone.load(Ordering::SeqCst) {
                    return;
                }
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let mut buf = [0u8; 4096];
                let n = match stream.read(&mut buf) {
                    Ok(n) if n > 0 => n,
                    _ => continue,
                };
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                // Strip query string for matching.
                let path_no_query = path
                    .split_once('?')
                    .map(|(p, _)| p)
                    .unwrap_or(&path)
                    .to_string();

                let s = state.lock().unwrap().clone();
                let archive_pattern = format!(
                    "/{}/{}/archive/refs/tags/{}.tar.gz",
                    s.org, s.name, s.version
                );
                let refs_pattern =
                    format!("/repos/{}/{}/git/refs/tags/{}", s.org, s.name, s.version);
                let tags_pattern = format!("/repos/{}/{}/tags", s.org, s.name);

                if path_no_query == archive_pattern {
                    write_binary_response(&mut stream, 200, "application/gzip", &s.tarball);
                } else if path_no_query == refs_pattern {
                    let body = format!(
                        "{{\"ref\":\"refs/tags/{}\",\"object\":{{\"sha\":\"{}\",\"type\":\"commit\",\"url\":\"u\"}}}}",
                        s.version, s.commit_sha
                    );
                    write_text_response(&mut stream, 200, "application/json", &body);
                } else if path_no_query == tags_pattern {
                    let body = format!("[{{\"name\":\"{}\"}}]", s.version);
                    write_text_response(&mut stream, 200, "application/json", &body);
                } else {
                    write_text_response(&mut stream, 404, "text/plain", "not found");
                }
                let _ = stream.shutdown(std::net::Shutdown::Both);
            }
        });

        MockServer {
            addr,
            stop,
            handle: Some(handle),
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn api_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop with a throwaway connection.
        let _ = std::net::TcpStream::connect(self.addr);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn write_text_response(
    stream: &mut std::net::TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) {
    let status_line = status_line_for(status);
    let resp = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status_line,
        content_type,
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn write_binary_response(
    stream: &mut std::net::TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) {
    let status_line = status_line_for(status);
    let header = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status_line,
        content_type,
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
}

fn status_line_for(status: u16) -> &'static str {
    match status {
        200 => "200 OK",
        404 => "404 Not Found",
        500 => "500 Internal Server Error",
        _ => "200 OK",
    }
}

// ---------------------------------------------------------------------------
// Tarball helpers
// ---------------------------------------------------------------------------

/// Build a gzipped tar archive containing the given files. Files are
/// placed under a top-level directory named `pkg-<timestamp>/` so the
/// `--strip-components=1` in `fetch_and_cache` flattens it into the
/// package directory. Gzipping is done by invoking `gzip` because
/// `taida` already depends on `tar` + `curl` being present.
///
/// Files must not be too large (this is for small test fixtures only).
pub fn make_tarball(files: &[(&str, &[u8])]) -> Vec<u8> {
    let tmp = std::env::temp_dir().join(format!(
        "taida_mock_tar_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let top = tmp.join("pkg-root");
    std::fs::create_dir_all(&top).unwrap();
    for (rel, data) in files {
        let p = top.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, data).unwrap();
    }
    // tar + gzip. We rely on system tar/gzip being installed (same
    // assumption as `GlobalStore::fetch_and_cache`).
    let tar_gz = tmp.join("pkg.tar.gz");
    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&tar_gz)
        .arg("-C")
        .arg(&tmp)
        .arg("pkg-root")
        .status()
        .expect("run tar");
    assert!(status.success(), "tar must succeed in make_tarball");
    let bytes = std::fs::read(&tar_gz).unwrap();
    let _ = std::fs::remove_dir_all(&tmp);
    bytes
}
