use std::path::PathBuf;

// TODO(T1.0): remove once `tag daemon` uses the isolated paths and control socket.
#[allow(dead_code)]
/// Filesystem locations used by a running TAG instance.
///
/// The UI uses [`AppPaths::user_default`]. `tag daemon` and tests override the state and socket
/// directories so they never touch the user's real session or sockets.
#[derive(Debug, Clone)]
pub(crate) struct AppPaths {
    /// `config.toml` path.
    pub(crate) config_path: PathBuf,
    /// Directory holding `config.toml`, prompts and the global brain.
    pub(crate) config_dir: PathBuf,
    /// Saved session (`session.json`).
    pub(crate) session_path: PathBuf,
    pub(crate) snippets_path: PathBuf,
    pub(crate) vault_path: PathBuf,
    /// State directory (runs, project registry, audit log, indexes).
    pub(crate) state_dir: PathBuf,
    /// Directory for the hook and control sockets.
    pub(crate) socket_dir: PathBuf,
}

#[allow(dead_code)]
impl AppPaths {
    /// Paths for the interactive UI, rooted in the user's home directory.
    pub(crate) fn user_default() -> Self {
        let config_dir = crate::session_io::dirs_or_home()
            .join(".config")
            .join("tag");
        Self {
            config_path: config_dir.join("config.toml"),
            session_path: config_dir.join("session.json"),
            snippets_path: config_dir.join("snippets.toml"),
            vault_path: config_dir.join("vault.enc"),
            state_dir: default_state_dir(),
            socket_dir: crate::sockets::get_tag_socket_path()
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(crate::sockets::preferred_socket_dir),
            config_dir,
        }
    }

    /// Paths for an isolated instance (daemon/tests): session lives in `state_dir`.
    pub(crate) fn isolated(
        config_path: Option<PathBuf>,
        state_dir: PathBuf,
        socket_dir: PathBuf,
    ) -> Self {
        let defaults = Self::user_default();
        let config_dir = config_path
            .as_ref()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| defaults.config_dir.clone());
        Self {
            config_path: config_path.unwrap_or(defaults.config_path),
            session_path: state_dir.join("session.json"),
            snippets_path: defaults.snippets_path,
            vault_path: defaults.vault_path,
            state_dir,
            socket_dir,
            config_dir,
        }
    }

    /// Hook socket path for this process.
    pub(crate) fn hook_socket_path(&self) -> PathBuf {
        self.socket_dir
            .join(format!("tag-sock-{}.sock", std::process::id()))
    }

    /// Control socket path for this process.
    pub(crate) fn control_socket_path(&self) -> PathBuf {
        self.socket_dir
            .join(format!("tag-control-{}.sock", std::process::id()))
    }
}

/// `$XDG_STATE_HOME/tag`, `~/.local/state/tag`, or on macOS `~/Library/Application Support/tag/state`.
pub(crate) fn default_state_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("tag");
        }
    }
    let home = crate::session_io::dirs_or_home();
    if cfg!(target_os = "macos") {
        home.join("Library")
            .join("Application Support")
            .join("tag")
            .join("state")
    } else {
        home.join(".local").join("state").join("tag")
    }
}
