//! Control socket discovery (§4.1): `$TAG_CONTROL_SOCKET`, else the newest live
//! `tag-control-<pid>.sock` in the per-user socket directories.

use std::path::{Path, PathBuf};

/// Env var naming the control socket explicitly.
pub const SOCKET_ENV: &str = "TAG_CONTROL_SOCKET";
/// Env var carrying a scoped control token.
pub const TOKEN_ENV: &str = "TAG_CONTROL_TOKEN";

/// File name of the control socket for a process.
pub fn socket_file_name(pid: u32) -> String {
    format!("tag-control-{pid}.sock")
}

/// Per-user directories that may hold TAG sockets, most preferred first.
pub fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(rt) = std::env::var("XDG_RUNTIME_DIR") {
        if !rt.is_empty() {
            dirs.push(PathBuf::from(rt).join("tag"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".config").join("tag").join("run"));
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    dirs.push(PathBuf::from(format!("/tmp/tag-{uid}")));
    dirs
}

/// Whether something is accepting connections at `path`.
pub fn is_live(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// Newest live control socket in `dirs` (by mtime).
pub fn newest_live_socket(dirs: &[PathBuf]) -> Option<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("tag-control-") && name.ends_with(".sock") {
                if let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) {
                    found.push((mtime, entry.path()));
                }
            }
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().map(|(_, p)| p).find(|p| is_live(p))
}

/// Resolve the control socket to connect to.
pub fn discover_socket() -> Option<PathBuf> {
    if let Ok(p) = std::env::var(SOCKET_ENV) {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    newest_live_socket(&candidate_dirs())
}
