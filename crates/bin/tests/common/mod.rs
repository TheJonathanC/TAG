//! Shared helpers for end-to-end tests that run `tag daemon` in a temp dir.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A `tag daemon` child process isolated from the user's real TAG, tmux and config.
pub struct Daemon {
    pub dir: tempfile::TempDir,
    pub child: Child,
    pub socket_dir: PathBuf,
    pub state_dir: PathBuf,
    pub home: PathBuf,
}

/// Env vars that would connect children to the real TAG instance running the tests.
pub const SCRUB: &[&str] = &[
    "TAG_ACTIVE",
    "TAG_PANE_ID",
    "COCKPIT_PANE_ID",
    "COCKPIT_SOCK",
    "TAG_CONTROL_SOCKET",
    "TAG_CONTROL_TOKEN",
    "TAG_RUN_ID",
    "TAG_TASK_ID",
    "TMUX",
    "TMUX_PANE",
    "XDG_RUNTIME_DIR",
];

/// A `Command` for the `tag` binary with a scrubbed, temp-rooted environment.
pub fn tag_cmd(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tag"));
    for k in SCRUB {
        cmd.env_remove(k);
    }
    cmd.env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"))
        .env("TMUX_TMPDIR", home.join("tmux"));
    cmd
}

impl Daemon {
    /// Start a daemon with an optional `config.toml` body.
    pub fn start(config: Option<&str>) -> Daemon {
        Self::start_with(config, |_| {})
    }

    /// Start a daemon after letting `prepare` populate the state dir (e.g. a session).
    pub fn start_with(config: Option<&str>, prepare: impl FnOnce(&Path)) -> Daemon {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let socket_dir = dir.path().join("s");
        let state_dir = dir.path().join("st");
        std::fs::create_dir_all(home.join("tmux")).unwrap();
        std::fs::create_dir_all(&state_dir).unwrap();
        prepare(&state_dir);
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, config.unwrap_or("")).unwrap();
        let child = tag_cmd(&home)
            .arg("daemon")
            .arg("--socket-dir")
            .arg(&socket_dir)
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("--config")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let d = Daemon {
            dir,
            child,
            socket_dir,
            state_dir,
            home,
        };
        d.wait_for(&d.control_socket());
        d
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn control_socket(&self) -> PathBuf {
        self.socket_dir
            .join(format!("tag-control-{}.sock", self.pid()))
    }

    pub fn hook_socket(&self) -> PathBuf {
        self.socket_dir
            .join(format!("tag-sock-{}.sock", self.pid()))
    }

    fn wait_for(&self, path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("daemon socket {} never came up", path.display());
    }

    /// Connect a control client as `client` (no token).
    pub fn client(&self, client: &str) -> tag_control::ControlClient {
        tag_control::ControlClient::connect(&self.control_socket(), client, None).unwrap()
    }

    /// Send SIGTERM and wait for a clean exit.
    pub fn shutdown(&mut self) -> std::process::ExitStatus {
        // SAFETY: signalling a child process this test started.
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("daemon did not exit after SIGTERM");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Write a session into `state_dir` with one shell pane (id 1) running `sh -c <script>`.
pub fn seed_shell_session(state_dir: &Path, script: &str) {
    use tag_core::pane::{PaneId, PaneKind, WorkspaceId};
    use tag_layout::{SavedPane, SavedSession, SavedWorkspace, Workspace};
    let mut ws = Workspace::new(WorkspaceId(0), "main");
    ws.add_initial_pane(PaneId(1));
    let mut s = SavedSession::new(WorkspaceId(0));
    s.workspaces.push(SavedWorkspace::from(&ws));
    s.panes.insert(
        PaneId(1),
        SavedPane {
            id: PaneId(1),
            title: "seed".into(),
            kind: PaneKind::Shell,
            cwd: Some(state_dir.to_path_buf()),
            command: Some("sh".into()),
            args: vec!["-c".into(), script.into()],
        },
    );
    s.save_to_file(&state_dir.join("session.json")).unwrap();
}

/// Poll `f` until it returns `Some` or `secs` pass.
pub fn eventually<T>(secs: u64, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f() {
            return v;
        }
        if Instant::now() > deadline {
            panic!("condition not met within {secs}s");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
