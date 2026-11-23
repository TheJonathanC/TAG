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
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let socket_dir = dir.path().join("s");
        let state_dir = dir.path().join("st");
        std::fs::create_dir_all(home.join("tmux")).unwrap();
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
