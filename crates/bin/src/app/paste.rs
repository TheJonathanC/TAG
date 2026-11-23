#[allow(unused_imports)]
use crate::{app::*, cli::*, session_io::*, sockets::*, tmux::*};
use std::path::PathBuf;
use tag_clipboard::{wrap_bracketed_paste, ImageBridge};
use tag_git::GitPromptMode;
use tag_vault::VaultMode;

pub(crate) fn handle_paste_image(file: Option<PathBuf>) -> Result<(), anyhow::Error> {
    use std::io::{Read, Write};
    let bytes = match file {
        Some(ref p) if p.as_os_str() != "-" => {
            if !p.exists() {
                return Err(anyhow::anyhow!("File not found: {:?}", p));
            }
            std::fs::read(p)?
        }
        _ => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf)?;
            if buf.is_empty() {
                return Err(anyhow::anyhow!(
                    "No input image data provided via stdin. Provide a file path or pipe image data."
                ));
            }
            buf
        }
    };

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let out_path = ImageBridge::save_image_bytes(&cwd, &bytes)?;

    let sock_path = if let Ok(s) = std::env::var("COCKPIT_SOCK") {
        if !s.is_empty() && std::path::Path::new(&s).exists() {
            Some(PathBuf::from(s))
        } else {
            find_latest_tag_socket()
        }
    } else {
        find_latest_tag_socket()
    };

    if let Some(sock) = sock_path {
        use std::os::unix::net::UnixStream;
        if let Ok(mut stream) = UnixStream::connect(&sock) {
            let payload = tag_core::event::HookPayload {
                event: tag_core::event::HookEvent::PasteImage,
                pane_id: None,
                agent: "image-bridge".into(),
                worktree: Some(cwd),
                message: out_path.to_string_lossy().to_string(),
                timestamp: chrono::Utc::now(),
            };
            if let Ok(json_bytes) = serde_json::to_vec(&payload) {
                let _ = stream.write_all(&json_bytes);
                let _ = stream.flush();
                println!("Pasted image into active TAG session: {:?}", out_path);
                return Ok(());
            }
        }
    }

    println!(
        "Saved image to {:?} (no running TAG socket found to auto-paste)",
        out_path
    );
    Ok(())
}

/// Send text to the focused pane as a paste (bracketed only if the program enabled it).
pub(crate) fn paste_into_focused(state: &mut AppState, text: &str) {
    if let Some(focused) = state.layout_mgr.active_workspace().focused_pane {
        if let Some(pane) = state.panes.get_mut(&focused) {
            pane.clear_selection();
            let bytes = if pane.bracketed_paste() {
                wrap_bracketed_paste(text)
            } else {
                text.as_bytes().to_vec()
            };
            pane.send_input(&bytes);
            pane.scroll_to_bottom();
            state.is_dirty = true;
        }
    }
}

/// Deliver pasted text to whichever input currently has focus: a text field in an
/// open overlay, or the focused pane when no overlay is open. Overlays without a
/// text field swallow the paste instead of sending it to the pane behind them.
pub(crate) fn route_paste(state: &mut AppState, text: &str) {
    state.is_dirty = true;
    if let Some(ref mut prompt) = state.rename_prompt {
        prompt.input.push_str(text);
    } else if state.palette.is_open {
        state.palette.query.push_str(text);
        state.palette.selected_index = 0;
    } else if state.help_open
        || state.tour.is_active
        || state.attention_state.is_open
        || state.usage_state.is_open
    {
        // no text input
    } else if state.git_state.is_open {
        if let Some(ref mut prompt) = state.git_state.prompt {
            if prompt.mode == GitPromptMode::CreateWorktree {
                prompt.input.push_str(text);
            }
        }
    } else if state.snippet_state.is_open {
        if let Some(ref mut fill) = state.snippet_state.fill {
            fill.input.push_str(text);
        }
    } else if state.vault_state.is_open {
        let accepts_text = !state.vault.is_unlocked()
            || matches!(
                state.vault_state.mode,
                VaultMode::AddingKey | VaultMode::AddingValue
            );
        if accepts_text {
            state.vault_state.input_buffer.push_str(text);
        }
    } else if state.tab_tray.is_open {
        state.tab_tray.search_query.push_str(text);
        state.tab_tray.is_searching = true;
        state.tab_tray.selected_index = 0;
    } else {
        paste_into_focused(state, text);
    }
}
