#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, tmux::*};
use std::path::PathBuf;

/// Unix socket paths are limited to ~104-108 bytes; keep a safety margin.
pub(crate) const MAX_SOCKET_PATH_LEN: usize = 100;

pub(crate) fn ensure_private_dir(dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
}

pub(crate) fn preferred_socket_dir() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("tag")
    } else {
        dirs_or_home().join(".config").join("tag").join("run")
    }
}

/// Short per-user fallback under /tmp, used only when the preferred path is too long
/// for a Unix socket. Rejected unless it is a real directory owned by us and private.
pub(crate) fn short_socket_dir() -> Option<PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let dir = PathBuf::from(format!("/tmp/tag-{}", uid));
    let _ = std::fs::create_dir(&dir);
    let meta = std::fs::symlink_metadata(&dir).ok()?;
    if !meta.is_dir() || meta.uid() != uid {
        return None;
    }
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    Some(dir)
}

pub(crate) fn socket_file_name() -> String {
    format!("tag-sock-{}.sock", std::process::id())
}

pub(crate) fn get_tag_socket_path() -> PathBuf {
    let dir = preferred_socket_dir();
    let path = dir.join(socket_file_name());
    if path.as_os_str().len() <= MAX_SOCKET_PATH_LEN {
        ensure_private_dir(&dir);
        return path;
    }
    match short_socket_dir() {
        Some(short) => short.join(socket_file_name()),
        None => {
            ensure_private_dir(&dir);
            path
        }
    }
}

pub(crate) fn find_latest_tag_socket() -> Option<PathBuf> {
    let mut best_sock = None;
    let mut best_time = std::time::SystemTime::UNIX_EPOCH;

    // Only private, per-user locations (never scan the shared temp dir)
    let mut candidate_dirs = vec![
        preferred_socket_dir(),
        dirs_or_home().join(".config").join("tag").join("run"),
    ];
    // SAFETY: getuid has no preconditions and cannot fail.
    let short = PathBuf::from(format!("/tmp/tag-{}", unsafe { libc::getuid() }));
    if short.exists() {
        candidate_dirs.extend(short_socket_dir());
    }

    for dir in candidate_dirs {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("tag-sock-") && name_str.ends_with(".sock") {
                    if let Ok(metadata) = entry.metadata() {
                        if let Ok(mtime) = metadata.modified() {
                            if mtime > best_time {
                                best_time = mtime;
                                best_sock = Some(entry.path());
                            }
                        }
                    }
                }
            }
        }
    }
    best_sock
}

pub(crate) fn is_inside_tag() -> bool {
    // 1. Check environment variables injected into child PTYs
    if std::env::var("TAG_ACTIVE").is_ok()
        || std::env::var("COCKPIT_PANE_ID").is_ok()
        || std::env::var("TAG_PANE_ID").is_ok()
    {
        return true;
    }

    // 2. Check process ancestor tree on Linux
    #[cfg(target_os = "linux")]
    {
        let mut current_pid = std::process::id();
        for _ in 0..15 {
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", current_pid)) {
                if let Some(rparen) = stat.rfind(')') {
                    let rest = &stat[rparen + 1..];
                    let fields: Vec<&str> = rest.split_whitespace().collect();
                    if fields.len() >= 2 {
                        if let Ok(ppid) = fields[1].parse::<u32>() {
                            if ppid <= 1 {
                                break;
                            }
                            if let Ok(comm) =
                                std::fs::read_to_string(format!("/proc/{}/comm", ppid))
                            {
                                if comm.trim() == "tag" {
                                    return true;
                                }
                            }
                            current_pid = ppid;
                            continue;
                        }
                    }
                }
            }
            break;
        }
    }

    false
}
