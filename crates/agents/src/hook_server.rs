use std::fs;
use std::os::unix::fs::PermissionsExt;
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
            // Set 0700 on private runtime directory
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }

        // Clean up stale socket file if present
        if self.socket_path.exists() {
            let _ = fs::remove_file(&self.socket_path);
        }

        let listener = UnixListener::bind(&self.socket_path)?;

        // Ensure 0600 on socket file
        let _ = fs::set_permissions(&self.socket_path, fs::Permissions::from_mode(0o600));
        info!("Hook socket server listening at {:?}", self.socket_path);

        tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((mut stream, _addr)) => {
                        let tx = event_tx.clone();
                        tokio::spawn(async move {
                            let mut buf = Vec::new();
                            if stream.read_to_end(&mut buf).await.is_ok() {
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
