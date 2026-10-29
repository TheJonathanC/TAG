use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;
use tokio::net::UnixListener;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{error, info};

use tag_core::event::{AppEvent, HookPayload};

pub struct HookServer {
    socket_path: PathBuf,
}

impl HookServer {
    pub fn new(socket_path: PathBuf) -> Self {
        Self { socket_path }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub async fn start(&self, event_tx: UnboundedSender<AppEvent>) -> Result<(), anyhow::Error> {
        // Ensure directory exists
        if let Some(parent) = self.socket_path.parent() {
            fs::create_dir_all(parent)?;
            // Set 0700 on private runtime directory (never on shared system dirs)
            if parent != Path::new("/tmp") && parent != Path::new("/var/tmp") {
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
            }
        }

        // Clean up stale socket file if present (remove_file does not follow symlinks)
        let _ = fs::remove_file(&self.socket_path);

        let listener = UnixListener::bind(&self.socket_path)?;

        // Ensure 0600 on socket file
        let _ = fs::set_permissions(&self.socket_path, fs::Permissions::from_mode(0o600));
        let owner_uid = fs::metadata(&self.socket_path).map(|m| m.uid()).ok();
        info!("Hook socket server listening at {:?}", self.socket_path);

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _addr)) => {
                        if let (Some(expected_uid), Ok(cred)) = (owner_uid, stream.peer_cred()) {
                            if cred.uid() != expected_uid {
                                error!(
                                    "Rejected hook socket connection from unauthorized UID {} (expected {})",
                                    cred.uid(),
                                    expected_uid
                                );
                                continue;
                            }
                        }
                        let tx = event_tx.clone();
                        tokio::spawn(async move {
                            let mut buf = Vec::new();
                            let mut limited = stream.take(1024 * 1024); // 1 MB limit to prevent memory exhaustion DoS
                                                                        // Clients write one payload and close; don't let a stuck client hold a task forever
                            let read = tokio::time::timeout(
                                std::time::Duration::from_secs(5),
                                limited.read_to_end(&mut buf),
                            )
                            .await;
                            if matches!(read, Ok(Ok(_))) {
                                match serde_json::from_slice::<HookPayload>(&buf) {
                                    Ok(payload) => {
                                        info!("Received hook event: {:?}", payload.event);
                                        let _ = tx.send(AppEvent::AgentHook(payload));
                                    }
                                    Err(e) => {
                                        error!("Failed to parse hook JSON payload: {}", e);
                                    }
                                }
                            }
                        });
                    }
                    Err(e) => {
                        error!("Hook listener accept error: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(())
    }
}
